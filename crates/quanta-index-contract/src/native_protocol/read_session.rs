//! Product read leases reference the Catalog-owned ticket retained by the daemon.

pub(super) use super::NativeReadSessionId;
use super::{NativeDslQueryV1, NativeSemanticChunk, NativeSemanticTraversal, NativeTextHit};
use crate::{
    FiniteScore, HistoryQueryRequest, RepoId, SemanticQueryRequest, TextQueryRequest,
    index::{
        CatalogView, CoverageBinding, Digest, HistoryGenerationInputV1, HistorySourceCutV1,
        IndexInstanceId, IndexProfile, ObjectRef, OwnerKey, ViewRef, ViewSelection,
    },
};

mod history;
pub use history::{
    NativeHistoryCursor, NativeHistoryPage, NativeHistoryPageRef, native_history_query_digest,
    validate_native_history_query,
};

mod ranking;
pub use ranking::{
    compare_native_text_path_rank_v1, native_text_query_digest, rank_native_hybrid_files,
    validate_native_hybrid_file_query,
};

#[cfg(test)]
mod wire_tests;

/// Shared maximum for the canonical bounded profile and its query reply.
pub const MAX_NATIVE_READ_PROFILE_INSTANCES: usize = 4096;

/// The daemon loads this profile object through the original retained ticket.
/// This is configuration metadata; it does not certify coverage or readiness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeReadSessionProfile {
    pub session: NativeReadSessionId,
    pub view: CatalogView,
    pub profile: IndexProfile,
}

quanta_index_contract_base::native_serde_struct!(NativeReadSessionProfile {
    session: NativeReadSessionId,
    view: CatalogView,
    profile: IndexProfile
});

/// The committed History cut and artifact selected by one retained view.
/// Reading this metadata does not issue a new source cut or advance the view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeReadSessionHistoryCutV1 {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub member: ObjectRef,
    pub cut: HistorySourceCutV1,
}

quanta_index_contract_base::native_serde_struct!(NativeReadSessionHistoryCutV1 {
    session: NativeReadSessionId,
    view: ViewRef,
    instance: IndexInstanceId,
    member: ObjectRef,
    cut: HistorySourceCutV1
});

/// The original and derived product inputs reachable from the History member
/// of an exact retained view. This is a read of published immutable outputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadSessionHistoryGenerationV1 {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub member: ObjectRef,
    pub source_input: ObjectRef,
    pub inputs: HistoryGenerationInputV1,
    /// Exact predecessor cut context retained by the committed successor.
    pub predecessor_context: Digest,
}
quanta_index_contract_base::native_serde_struct!(NativeReadSessionHistoryGenerationV1 {
    session: NativeReadSessionId,
    view: ViewRef,
    instance: IndexInstanceId,
    member: ObjectRef,
    source_input: ObjectRef,
    inputs: HistoryGenerationInputV1,
    predecessor_context: Digest
});

/// Stateless rank anchor; the session still supplies the original read authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeTextCursor {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub query: Digest,
    pub artifact: ObjectRef,
    pub score: FiniteScore,
    pub owner: OwnerKey,
}

/// Stateless rank anchor. The caller must still hold a session for the view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextPathCursorV1 {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub query: Digest,
    pub artifact: ObjectRef,
    pub score: FiniteScore,
    pub repository: RepoId,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextPage {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub artifact: ObjectRef,
    pub matched: u64,
    /// Full matches after the anchor, before the current page cap.
    pub remaining: u64,
    pub hits: Vec<NativeTextHit>,
    pub after: Option<NativeTextCursor>,
}

/// Authenticated source identity, separate from the retained vector witness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSemanticFileHit {
    pub owner: OwnerKey,
    pub source: ObjectRef,
    pub path: String,
    pub projection_owner: OwnerKey,
    pub vector_id: Vec<u8>,
    pub chunk: Option<NativeSemanticChunk>,
    pub score: FiniteScore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeHybridFileHit {
    pub owner: OwnerKey,
    /// One-based ranks into the complete bounded file pools below.
    pub lexical_rank: Option<u32>,
    pub dense_rank: Option<u32>,
    pub score: FiniteScore,
}

/// Bounded RRF over independent same-view file pools. This is never a
/// generation-wide exhaustive Hybrid ranking certificate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeHybridFileResult {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub text_instance: IndexInstanceId,
    pub semantic_instance: IndexInstanceId,
    pub lexical_artifact: ObjectRef,
    pub lexical_matched: u64,
    pub semantic_coverage: CoverageBinding,
    pub semantic_traversal: NativeSemanticTraversal,
    pub semantic_exhaustive: bool,
    pub scanned_owners: u32,
    pub skipped_stale: u32,
    pub file_top_k: u32,
    pub lexical_pool: Vec<NativeTextHit>,
    pub dense_pool: Vec<NativeSemanticFileHit>,
    pub hits: Vec<NativeHybridFileHit>,
}

quanta_index_contract_base::native_serde_struct!(NativeTextCursor {
    view: ViewRef,
    instance: IndexInstanceId,
    query: Digest,
    artifact: ObjectRef,
    score: FiniteScore,
    owner: OwnerKey
});
quanta_index_contract_base::native_serde_struct!(NativeTextPathCursorV1 {
    view: ViewRef,
    instance: IndexInstanceId,
    query: Digest,
    artifact: ObjectRef,
    score: FiniteScore,
    repository: RepoId,
    path: String
});
quanta_index_contract_base::native_serde_struct!(NativeTextPage { session: NativeReadSessionId, view: ViewRef, instance: IndexInstanceId, artifact: ObjectRef, matched: u64, remaining: u64, hits: Vec<NativeTextHit>, after: Option<NativeTextCursor> });
quanta_index_contract_base::native_serde_struct!(NativeSemanticFileHit { owner: OwnerKey, source: ObjectRef, path: String, projection_owner: OwnerKey, vector_id: Vec<u8>, chunk: Option<NativeSemanticChunk>, score: FiniteScore });
quanta_index_contract_base::native_serde_struct!(NativeHybridFileHit { owner: OwnerKey, lexical_rank: Option<u32>, dense_rank: Option<u32>, score: FiniteScore });
quanta_index_contract_base::native_serde_struct!(NativeHybridFileResult { session: NativeReadSessionId, view: ViewRef, text_instance: IndexInstanceId, semantic_instance: IndexInstanceId, lexical_artifact: ObjectRef, lexical_matched: u64, semantic_coverage: CoverageBinding, semantic_traversal: NativeSemanticTraversal, semantic_exhaustive: bool, scanned_owners: u32, skipped_stale: u32, file_top_k: u32, lexical_pool: Vec<NativeTextHit>, dense_pool: Vec<NativeSemanticFileHit>, hits: Vec<NativeHybridFileHit> });

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReadSessionState {
    Active,
    Closing,
    Released,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadSessionStatus {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub expires_at_ms: u64,
    pub revision: u64,
    pub state: NativeReadSessionState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReadSettlementState {
    Pending,
    Incomplete,
    SettledAtSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadSessionSettlement {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub head: ViewRef,
    pub exact_floor: u64,
    pub state: NativeReadSettlementState,
    /// Remaining target pins in this settlement snapshot, rather than at release.
    pub has_remaining_target_pins: bool,
}

#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "Read-session queries remain inline in the guarded request and borrowed caller DTO; boxing adds an unadmitted allocation, while replacing query types changes the established caller and strict codec shape"
)]
pub enum NativeReadSessionRequest {
    Open {
        selection: ViewSelection,
        open_key: [u8; 32],
    },
    Renew {
        session: NativeReadSessionId,
        revision: u64,
    },
    Status(NativeReadSessionId),
    ReadProfile {
        session: NativeReadSessionId,
    },
    ReadHistoryCut {
        session: NativeReadSessionId,
        instance: IndexInstanceId,
    },
    ReadHistoryGeneration {
        session: NativeReadSessionId,
        instance: IndexInstanceId,
    },
    SearchText {
        session: NativeReadSessionId,
        instance: IndexInstanceId,
        query: TextQueryRequest,
    },
    SearchTextPage {
        session: NativeReadSessionId,
        instance: IndexInstanceId,
        query: TextQueryRequest,
        after: Option<NativeTextCursor>,
    },
    SearchTextPathPageV1 {
        session: NativeReadSessionId,
        instance: IndexInstanceId,
        query: TextQueryRequest,
        after: Option<NativeTextPathCursorV1>,
    },
    SearchHybridFile {
        session: NativeReadSessionId,
        text_instance: IndexInstanceId,
        semantic_instance: IndexInstanceId,
        text_query: TextQueryRequest,
        semantic_query: SemanticQueryRequest,
        file_top_k: u32,
    },
    SearchHistory {
        session: NativeReadSessionId,
        instance: IndexInstanceId,
        query: HistoryQueryRequest,
        after: Option<NativeHistoryCursor>,
    },
    SearchDslV1 {
        session: NativeReadSessionId,
        query: NativeDslQueryV1,
    },
    Release(NativeReadSessionId),
    Settlement(NativeReadSessionId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReadSessionReply {
    Status(NativeReadSessionStatus),
    Released {
        status: NativeReadSessionStatus,
        has_remaining_target_pins: bool,
    },
    Settlement(NativeReadSessionSettlement),
}

quanta_index_contract_base::native_serde_enum!(NativeReadSessionState { units { Active = 0, Closing = 1, Released = 2, Unknown = 3 } newtypes { } records { } });
quanta_index_contract_base::native_serde_enum!(NativeReadSettlementState { units { Pending = 0, Incomplete = 1, SettledAtSnapshot = 2 } newtypes { } records { } });
quanta_index_contract_base::native_serde_struct!(NativeReadSessionStatus {
    session: NativeReadSessionId,
    view: ViewRef,
    expires_at_ms: u64,
    revision: u64,
    state: NativeReadSessionState
});
quanta_index_contract_base::native_serde_struct!(NativeReadSessionSettlement {
    session: NativeReadSessionId,
    view: ViewRef,
    head: ViewRef,
    exact_floor: u64,
    state: NativeReadSettlementState,
    has_remaining_target_pins: bool
});
quanta_index_contract_base::native_serde_enum!(NativeReadSessionRequest { units { } newtypes { Status = 2 => NativeReadSessionId, Release = 4 => NativeReadSessionId, Settlement = 5 => NativeReadSessionId } records { Open = 0 { selection: ViewSelection, open_key: [u8; 32] }, Renew = 1 { session: NativeReadSessionId, revision: u64 }, ReadProfile = 8 { session: NativeReadSessionId }, SearchText = 3 { session: NativeReadSessionId, instance: IndexInstanceId, query: TextQueryRequest }, SearchTextPage = 6 { session: NativeReadSessionId, instance: IndexInstanceId, query: TextQueryRequest, after: Option<NativeTextCursor> }, SearchHybridFile = 7 { session: NativeReadSessionId, text_instance: IndexInstanceId, semantic_instance: IndexInstanceId, text_query: TextQueryRequest, semantic_query: SemanticQueryRequest, file_top_k: u32 }, SearchHistory = 9 { session: NativeReadSessionId, instance: IndexInstanceId, query: HistoryQueryRequest, after: Option<NativeHistoryCursor> }, ReadHistoryCut = 10 { session: NativeReadSessionId, instance: IndexInstanceId }, ReadHistoryGeneration = 11 { session: NativeReadSessionId, instance: IndexInstanceId }, SearchTextPathPageV1 = 12 { session: NativeReadSessionId, instance: IndexInstanceId, query: TextQueryRequest, after: Option<NativeTextPathCursorV1> }, SearchDslV1 = 16 { session: NativeReadSessionId, query: NativeDslQueryV1 } } });
quanta_index_contract_base::native_serde_enum!(NativeReadSessionReply { units { } newtypes { Status = 0 => NativeReadSessionStatus, Settlement = 2 => NativeReadSessionSettlement } records { Released = 1 { status: NativeReadSessionStatus, has_remaining_target_pins: bool } } });

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_protocol::NativeIndexRequest;

    #[test]
    fn read_session_release_has_fixed_public_wire_shape() -> Result<(), Box<dyn std::error::Error>>
    {
        let expected = serde_json::json!({"ReadSession": {"Release": vec![7_u8; 32]}});
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::Release(
            NativeReadSessionId([7; 32]),
        ));
        assert_eq!(serde_json::to_value(&request)?, expected);
        assert_eq!(
            serde_json::from_value::<NativeIndexRequest>(expected)?,
            request
        );
        Ok(())
    }

    #[test]
    fn read_session_handle_decode_refuses_wrong_length_and_unknown_operation() {
        for length in [0, 31, 33] {
            let wire = serde_json::json!({"ReadSession": {"Release": vec![7_u8; length]}});
            assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
        }
        let wire = serde_json::json!({"ReadSession": {"ForceRelease": vec![7_u8; 32]}});
        assert!(serde_json::from_value::<NativeIndexRequest>(wire).is_err());
    }
}
