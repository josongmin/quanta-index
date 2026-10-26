//! Canonical headless editor overlay wire. Authentication comes from the
//! transport's kernel peer and live publisher lease, never these DTO fields.

use crate::index::{
    Digest, IndexInstanceId, ObjectRef, OperationKey, OwnerKey, UpdateRequest, ViewRef,
    WorkspaceKey,
};

use super::{NativeIndexResponse, NativeInteractionPublisherId};
use crate::{FiniteScore, TextQueryRequest, index::ViewSelection};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeOverlaySession {
    pub workspace: WorkspaceKey,
    pub publisher: NativeInteractionPublisherId,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeOverlayDocument {
    pub session: NativeOverlaySession,
    pub owner: OwnerKey,
}

/// The runtime must authenticate this operation against the original source
/// CAS and check the exact selected owner frontier before retiring text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeOverlaySaveReceipt {
    pub operation: OperationKey,
    pub document: NativeOverlayDocument,
    pub revision: u64,
    pub source: Option<ObjectRef>,
    pub selected: ViewRef,
}

/// Bind the original source CAS before submission.
///
/// Operation and base view come from the original request; they are never
/// duplicated as independently forgeable identity fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOverlaySaveIntent {
    pub document: NativeOverlayDocument,
    pub revision: u64,
    pub expected_source: Option<ObjectRef>,
    pub request: UpdateRequest,
}

impl NativeOverlaySaveIntent {
    #[must_use]
    pub fn operation(&self) -> OperationKey {
        self.request.operation
    }
    #[must_use]
    pub fn base_view(&self) -> ViewRef {
        self.request.expected_view
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeOverlayRequest {
    Begin {
        session: NativeOverlaySession,
    },
    Update {
        document: NativeOverlayDocument,
        expected_revision: Option<u64>,
        revision: u64,
        text: Option<String>,
        path: Option<String>,
    },
    Snapshot {
        document: NativeOverlayDocument,
    },
    Masks {
        session: NativeOverlaySession,
    },
    Close {
        session: NativeOverlaySession,
    },
}

/// `None` overlay means no mask; `Some` with `text=None` masks a durable row
/// as an unsaved deletion. The distinction is preserved on the wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOverlaySnapshot {
    pub revision: u64,
    pub text: Option<String>,
    pub path: Option<String>,
}

/// One unsaved owner mask. `deleted` masks the selected durable owner without
/// replacement text; callers fetch a live revision with `Snapshot` as needed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeOverlayMask {
    pub owner: OwnerKey,
    pub revision: u64,
    pub deleted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeOverlayReply {
    Begun(NativeOverlaySession),
    Updated {
        document: NativeOverlayDocument,
        revision: u64,
    },
    Snapshot {
        document: NativeOverlayDocument,
        overlay: Option<NativeOverlaySnapshot>,
    },
    Masks {
        session: NativeOverlaySession,
        masks: Vec<NativeOverlayMask>,
    },
    SaveBound {
        document: NativeOverlayDocument,
        revision: u64,
        operation: OperationKey,
    },
    Saved {
        document: NativeOverlayDocument,
        revision: u64,
        selected: ViewRef,
    },
    Closed(NativeOverlaySession),
}

quanta_index_contract_base::native_serde_struct!(NativeOverlaySession {
    workspace: WorkspaceKey,
    publisher: NativeInteractionPublisherId,
    epoch: u64
});
quanta_index_contract_base::native_serde_struct!(NativeOverlayDocument {
    session: NativeOverlaySession,
    owner: OwnerKey
});
quanta_index_contract_base::native_serde_struct!(NativeOverlaySaveReceipt {
    operation: OperationKey,
    document: NativeOverlayDocument,
    revision: u64,
    source: Option<ObjectRef>,
    selected: ViewRef
});
quanta_index_contract_base::native_serde_struct!(NativeOverlaySaveIntent {
    document: NativeOverlayDocument,
    revision: u64,
    expected_source: Option<ObjectRef>,
    request: UpdateRequest
});
quanta_index_contract_base::native_serde_enum!(NativeOverlayRequest {
    units {}
    newtypes {}
    records {
        Begin = 0 { session: NativeOverlaySession },
        Update = 1 { document: NativeOverlayDocument, expected_revision: Option<u64>, revision: u64, text: Option<String>, path: Option<String> },
        Snapshot = 2 { document: NativeOverlayDocument },
        Close = 4 { session: NativeOverlaySession },
        Masks = 6 { session: NativeOverlaySession }
    }
});
quanta_index_contract_base::native_serde_struct!(NativeOverlaySnapshot {
    revision: u64,
    text: Option<String>,
    path: Option<String>
});
quanta_index_contract_base::native_serde_struct!(NativeOverlayMask {
    owner: OwnerKey,
    revision: u64,
    deleted: bool
});
quanta_index_contract_base::native_serde_enum!(NativeOverlayReply {
    units {}
    newtypes { Begun = 0 => NativeOverlaySession, Closed = 4 => NativeOverlaySession }
    records {
        Updated = 1 { document: NativeOverlayDocument, revision: u64 },
        Snapshot = 2 { document: NativeOverlayDocument, overlay: Option<NativeOverlaySnapshot> },
        Saved = 3 { document: NativeOverlayDocument, revision: u64, selected: ViewRef },
        SaveBound = 5 { document: NativeOverlayDocument, revision: u64, operation: OperationKey },
        Masks = 6 { session: NativeOverlaySession, masks: Vec<NativeOverlayMask> }
    }
});

impl NativeIndexResponse {
    /// Borrow the canonical reply while runtime allocation custody remains
    /// live through synchronous serialization.
    pub fn serialize_overlay_ref<S: serde::Serializer>(
        reply: &NativeOverlayReply,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 25, "Overlay", reply)
    }
}

/// Unsaved hits carry a revision, never a fabricated immutable source reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOverlayTextSource {
    Durable(ObjectRef),
    Overlay { revision: u64 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeOverlayRevision {
    pub owner: OwnerKey,
    pub revision: u64,
    /// The captured text is absent. Retired watermarks always have this set.
    pub deleted: bool,
    pub retired: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOverlayCorpusStamp {
    pub session: NativeOverlaySession,
    pub identity: Digest,
    pub revisions: Vec<NativeOverlayRevision>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOverlayTextHit {
    pub owner: OwnerKey,
    pub source: NativeOverlayTextSource,
    pub path: String,
    pub score: FiniteScore,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOverlayTextResult {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub artifact: ObjectRef,
    pub matched: u64,
    pub exhaustive: bool,
    pub corpus: NativeOverlayCorpusStamp,
    pub hits: Vec<NativeOverlayTextHit>,
}
quanta_index_contract_base::native_serde_enum!(NativeOverlayTextSource {
    units {} newtypes { Durable = 0 => ObjectRef }
    records { Overlay = 1 { revision: u64 } }
});
quanta_index_contract_base::native_serde_struct!(NativeOverlayRevision {
    owner: OwnerKey,
    revision: u64,
    deleted: bool,
    retired: bool
});
quanta_index_contract_base::native_serde_struct!(NativeOverlayCorpusStamp {
    session: NativeOverlaySession, identity: Digest, revisions: Vec<NativeOverlayRevision>
});
quanta_index_contract_base::native_serde_struct!(NativeOverlayTextHit {
    owner: OwnerKey,
    source: NativeOverlayTextSource,
    path: String,
    score: FiniteScore
});
quanta_index_contract_base::native_serde_struct!(NativeOverlayTextResult {
    view: ViewRef, instance: IndexInstanceId, artifact: ObjectRef, matched: u64,
    exhaustive: bool, corpus: NativeOverlayCorpusStamp, hits: Vec<NativeOverlayTextHit>
});
impl NativeIndexResponse {
    pub fn serialize_overlay_text_ref<S: serde::Serializer>(
        result: &NativeOverlayTextResult,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 28, "OverlayTextRanked", result)
    }
}

/// The original selected view and overlay stamp travel together.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOverlayTextQuery {
    pub selection: ViewSelection,
    pub instance: IndexInstanceId,
    pub session: NativeOverlaySession,
    pub query: TextQueryRequest,
    /// Fences the unsaved corpus only. Use `selection=Exact` to also fence the
    /// durable selected view; `Latest` intentionally selects its current view.
    pub expected_corpus: Option<Digest>,
}
quanta_index_contract_base::native_serde_struct!(NativeOverlayTextQuery {
    selection: ViewSelection, instance: IndexInstanceId, session: NativeOverlaySession,
    query: TextQueryRequest, expected_corpus: Option<Digest>
});

/// Authenticated by the transport; the payload alone grants no publisher right.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeOverlayEffortContext {
    pub session: NativeOverlaySession,
    pub expected_corpus: Option<Digest>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeOverlaySemanticChunk {
    pub id: Digest,
    pub input_digest: Digest,
    pub start: u64,
    pub end: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeOverlaySemanticHitOrigin {
    pub revision: u64,
    pub chunk: Option<NativeOverlaySemanticChunk>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOverlaySemanticHit {
    pub owner: OwnerKey,
    pub vector_id: Vec<u8>,
    pub score: FiniteScore,
    pub chunk: Option<super::NativeSemanticChunk>,
    pub overlay: Option<NativeOverlaySemanticHitOrigin>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOverlaySemanticResult {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    /// Coverage of the selected durable semantic member. The corpus stamp
    /// separately binds unsaved contributions and never rewrites this coverage.
    pub coverage: crate::index::CoverageBinding,
    pub traversal: super::NativeSemanticTraversal,
    pub exhaustive: bool,
    pub scanned_owners: u32,
    pub skipped_stale: u32,
    pub corpus: NativeOverlayCorpusStamp,
    pub hits: Vec<NativeOverlaySemanticHit>,
}
quanta_index_contract_base::native_serde_struct!(NativeOverlayEffortContext {
    session: NativeOverlaySession, expected_corpus: Option<Digest>
});
quanta_index_contract_base::native_serde_struct!(NativeOverlaySemanticChunk {
    id: Digest,
    input_digest: Digest,
    start: u64,
    end: u64
});
quanta_index_contract_base::native_serde_struct!(NativeOverlaySemanticHitOrigin {
    revision: u64, chunk: Option<NativeOverlaySemanticChunk>
});
quanta_index_contract_base::native_serde_struct!(NativeOverlaySemanticHit {
    owner: OwnerKey, vector_id: Vec<u8>, score: FiniteScore,
    chunk: Option<super::NativeSemanticChunk>, overlay: Option<NativeOverlaySemanticHitOrigin>
});
quanta_index_contract_base::native_serde_struct!(NativeOverlaySemanticResult {
    view: ViewRef, instance: IndexInstanceId, coverage: crate::index::CoverageBinding,
    traversal: super::NativeSemanticTraversal,
    exhaustive: bool, scanned_owners: u32, skipped_stale: u32,
    corpus: NativeOverlayCorpusStamp, hits: Vec<NativeOverlaySemanticHit>
});
impl NativeIndexResponse {
    pub fn serialize_overlay_semantic_ref<S: serde::Serializer>(
        result: &NativeOverlaySemanticResult,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant(
            "NativeIndexResponse",
            29,
            "OverlaySemanticRanked",
            result,
        )
    }
    pub fn serialize_effort_done_ref<S: serde::Serializer>(
        result: &super::NativeEffortDone,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 19, "EffortDone", result)
    }
}
