//! Bounded syntax admission for the retained Native cutover evidence file.
//!
//! This is a data transport, not a publication receipt or selection permit.
//! The receiver must join its typed facts with the live Catalog and deployment
//! authority while holding the actual root lease.

use crate::index::{
    Digest, HistorySourceCutV1, IndexInstanceId, NativeImportedSourceHeadV1, ObjectRef, OperationKey, ProducerId, Refusal, SourceCut, SourceOperationMode,
    SourceTextAdmission, StreamId, UpdateRequest, ViewRef, WorkspaceKey,
};
pub use quanta_index_contract_base::NativeCutoverDigest64V1;

#[path = "native_cutover_evidence_v1/before_selection_v1.rs"]
mod before_selection_v1;
pub use before_selection_v1::read_native_cutover_before_native_selector_v1;

/// FD6 is a private, pinned, read-only CBOR file. Refuse before decoding any
/// record when its physical length exceeds this limit.
pub const MAX_NATIVE_CUTOVER_EVIDENCE_BYTES_V1: usize = 1 << 20;
pub const MAX_NATIVE_CUTOVER_CONTROL_BYTES_V1: usize = 8192;
pub const MAX_NATIVE_CUTOVER_EVIDENCE_TEXT_BYTES_V1: usize = 4096;
pub const MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1: usize = 4096;
pub const MAX_NATIVE_CUTOVER_EVIDENCE_MAP_FIELDS_V1: usize = 64;
const MAX_DEPTH_V1: usize = 16;

/// The descriptor numbers themselves are fixed by the child transport; the
/// device/inode pair binds the borrowed original Files to the evidence header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceFileIdentityV1 {
    pub device: u64,
    pub inode: u64,
}
crate::native_serde_struct!(NativeCutoverEvidenceFileIdentityV1 {
    device: u64,
    inode: u64
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCutoverHostWatchModeV1 {
    Polling,
    Watchman,
    External,
}
crate::native_serde_enum!(NativeCutoverHostWatchModeV1 {
    units { Polling = 0, Watchman = 1, External = 2 }
    newtypes {}
    records {}
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCutoverHostCustodyModeV1 {
    EphemeralRescan,
    DurableInbox,
}
crate::native_serde_enum!(NativeCutoverHostCustodyModeV1 {
    units { EphemeralRescan = 0, DurableInbox = 1 }
    newtypes {}
    records {}
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceHostSealV1 {
    pub canonical_workspace_root: String,
    pub host_instance: u64,
    pub host_generation: u64,
    pub watch_mode: NativeCutoverHostWatchModeV1,
    pub custody_mode: NativeCutoverHostCustodyModeV1,
    pub namespace_root: String,
    pub namespace_file: NativeCutoverEvidenceFileIdentityV1,
}
crate::native_serde_struct!(NativeCutoverEvidenceHostSealV1 {
    canonical_workspace_root: String,
    host_instance: u64,
    host_generation: u64,
    watch_mode: NativeCutoverHostWatchModeV1,
    custody_mode: NativeCutoverHostCustodyModeV1,
    namespace_root: String,
    namespace_file: NativeCutoverEvidenceFileIdentityV1
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceSelectedSourceV1 {
    pub workspace: WorkspaceKey,
    pub workspace_epoch: u64,
    pub producer: ProducerId,
    pub stream: StreamId,
    pub stream_epoch: u64,
    pub text_admission: SourceTextAdmission,
}
crate::native_serde_struct!(NativeCutoverEvidenceSelectedSourceV1 {
    workspace: WorkspaceKey,
    workspace_epoch: u64,
    producer: ProducerId,
    stream: StreamId,
    stream_epoch: u64,
    text_admission: SourceTextAdmission
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceHeaderV1 {
    pub format_version: u32,
    pub canonical_workspace_root: String,
    pub repo_id: String,
    pub contract_key: String,
    pub writer_state_root: String,
    pub canonical_snapshot_namespace: String,
    pub sdk_lock_path: String,
    pub config_path: String,
    pub config_sha256: Digest,
    pub original_full_operation: OperationKey,
    pub selected_source: NativeCutoverEvidenceSelectedSourceV1,
    pub host: NativeCutoverEvidenceHostSealV1,
    pub sdk_lock_file: NativeCutoverEvidenceFileIdentityV1,
    pub current_selection_sha256: Digest,
    pub candidate_selection_sha256: Digest,
}
crate::native_serde_struct!(NativeCutoverEvidenceHeaderV1 {
    format_version: u32,
    canonical_workspace_root: String,
    repo_id: String,
    contract_key: String,
    writer_state_root: String,
    canonical_snapshot_namespace: String,
    sdk_lock_path: String,
    config_path: String,
    config_sha256: Digest,
    original_full_operation: OperationKey,
    selected_source: NativeCutoverEvidenceSelectedSourceV1,
    host: NativeCutoverEvidenceHostSealV1,
    sdk_lock_file: NativeCutoverEvidenceFileIdentityV1,
    current_selection_sha256: Digest,
    candidate_selection_sha256: Digest
});

/// An authenticated signed-store target. `expected_source` is the prior CAS
/// cut; a Q receiver must independently read the actually Published `SourceCut`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceTargetV1 {
    pub operation: OperationKey,
    pub expected_view: ViewRef,
    pub expected_source: SourceCut,
    pub expected_profile: ObjectRef,
    pub source_mode: SourceOperationMode,
    pub event_predecessor: u64,
    pub event_sequence: u64,
    pub owner_identity_policy_digest: Digest,
    pub text_admission: SourceTextAdmission,
    pub canonical_target_digest: Digest,
}
crate::native_serde_struct!(NativeCutoverEvidenceTargetV1 {
    operation: OperationKey,
    expected_view: ViewRef,
    expected_source: SourceCut,
    expected_profile: ObjectRef,
    source_mode: SourceOperationMode,
    event_predecessor: u64,
    event_sequence: u64,
    owner_identity_policy_digest: Digest,
    text_admission: SourceTextAdmission,
    canonical_target_digest: Digest
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeCutoverReservationEvidenceV1 {
    Initial,
    Watch {
        idempotency_key: String,
        verified_batch_digest: Digest,
        prior_target_digest: Digest,
    },
    Sdk {
        identity_kind: String,
        attempt_identity_digest: NativeCutoverDigest64V1,
        routing_identity_digest: NativeCutoverDigest64V1,
        prior_target_digest: Digest,
    },
}
crate::native_serde_enum!(NativeCutoverReservationEvidenceV1 {
    units { Initial = 0 }
    newtypes {}
    records {
        Watch = 1 {
            idempotency_key: String,
            verified_batch_digest: Digest,
            prior_target_digest: Digest
        },
        Sdk = 2 {
            identity_kind: String,
            attempt_identity_digest: NativeCutoverDigest64V1,
            routing_identity_digest: NativeCutoverDigest64V1,
            prior_target_digest: Digest
        }
    }
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCutoverReplayPhaseV1 {
    Preparing,
    Prepared,
    ReadyToActivate,
    Committed,
    Dispatching,
    Terminal,
    Acked,
    CleanupFencing,
    CleanupPending,
    Cleaned,
    RecoveryRequired,
}
crate::native_serde_enum!(NativeCutoverReplayPhaseV1 {
    units {
        Preparing = 0, Prepared = 1, ReadyToActivate = 2, Committed = 3,
        Dispatching = 4, Terminal = 5, Acked = 6, CleanupFencing = 7,
        CleanupPending = 8, Cleaned = 9, RecoveryRequired = 10
    }
    newtypes {}
    records {}
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCutoverAstWorkPhaseV1 {
    Queued,
    Claimed,
    Bound,
    Published,
    Refused,
    Superseded,
}
crate::native_serde_enum!(NativeCutoverAstWorkPhaseV1 {
    units {
        Queued = 0, Claimed = 1, Bound = 2,
        Published = 3, Refused = 4, Superseded = 5
    }
    newtypes {}
    records {}
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCutoverAttemptFenceWireV1 {
    LegacyUnfenced,
    BoundV2 {
        immutable_owner_token: u64,
        workspace_identity_digest: NativeCutoverDigest64V1,
        runtime_generation: u64,
    },
}
crate::native_serde_enum!(NativeCutoverAttemptFenceWireV1 {
    units { LegacyUnfenced = 0 }
    newtypes {}
    records {
        BoundV2 = 1 {
            immutable_owner_token: u64,
            workspace_identity_digest: NativeCutoverDigest64V1,
            runtime_generation: u64
        }
    }
});

/// Actual outbox publish binding. Dirty-track rows retain the auxiliary
/// digest in the historical `manifest_digest_hex` field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceOutboxPublishBindingV1 {
    pub generation: u64,
    pub manifest_digest_hex: String,
}
crate::native_serde_struct!(NativeCutoverEvidenceOutboxPublishBindingV1 {
    generation: u64,
    manifest_digest_hex: String
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceOutboxSemanticAdmissionMetricsV1 {
    pub admitted_symbol_card_count: u32,
    pub excluded_generated_source_count: u32,
    pub excluded_vendor_source_count: u32,
    pub excluded_trivial_accessor_count: u32,
    pub excluded_admission_facts_unavailable_count: u32,
    pub excluded_unsupported_identity_count: u32,
    pub raw_fallback_scope_count: u32,
    pub raw_fallback_token_count: u64,
    pub total_code_token_count: u64,
    pub raw_fallback_ratio_bps: u16,
}
crate::native_serde_struct!(NativeCutoverEvidenceOutboxSemanticAdmissionMetricsV1 {
    admitted_symbol_card_count: u32,
    excluded_generated_source_count: u32,
    excluded_vendor_source_count: u32,
    excluded_trivial_accessor_count: u32,
    excluded_admission_facts_unavailable_count: u32,
    excluded_unsupported_identity_count: u32,
    raw_fallback_scope_count: u32,
    raw_fallback_token_count: u64,
    total_code_token_count: u64,
    raw_fallback_ratio_bps: u16
});

/// Retained receipt DATA. The signed Source and live destination must verify
/// its publication binding independently before granting cutover authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceOutboxDeliveryReceiptV1 {
    pub generation: u64,
    pub manifest_digest_hex: String,
    pub accepted_replace_scopes: u32,
    pub accepted_tombstone_scopes: u32,
    pub accepted_semantic_replace_scopes: u32,
    pub accepted_semantic_tombstone_scopes: u32,
    pub sealed: bool,
    pub semantic_admission_metrics: Option<NativeCutoverEvidenceOutboxSemanticAdmissionMetricsV1>,
}
crate::native_serde_struct!(NativeCutoverEvidenceOutboxDeliveryReceiptV1 {
    generation: u64,
    manifest_digest_hex: String,
    accepted_replace_scopes: u32,
    accepted_tombstone_scopes: u32,
    accepted_semantic_replace_scopes: u32,
    accepted_semantic_tombstone_scopes: u32,
    sealed: bool,
    semantic_admission_metrics: Option<NativeCutoverEvidenceOutboxSemanticAdmissionMetricsV1>
});

/// The existing producer's committed Source association. Ordinary Replay and
/// Native initial and completed Watch facts reuse these exact axes and wire.
/// Terminal/ACK and Q publication identities remain on their existing facts.
/// Decoding this tuple never proves that its producer or publication is genuine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceSourcePublicationBindingV1 {
    pub transition_digest: NativeCutoverDigest64V1,
    pub completion_pointer_digest: NativeCutoverDigest64V1,
    pub physical_target_source_digest: NativeCutoverDigest64V1,
    pub target_snapshot_id: u64,
}
crate::native_serde_struct!(NativeCutoverEvidenceSourcePublicationBindingV1 {
    transition_digest: NativeCutoverDigest64V1,
    completion_pointer_digest: NativeCutoverDigest64V1,
    physical_target_source_digest: NativeCutoverDigest64V1,
    target_snapshot_id: u64
});

/// Projections retain each actual category and its binding axes. A receiver
/// that has no semantic join for a category refuses it explicitly.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "The bounded by-value SourceFact is charged by size_of in the Original evidence projector; boxing AstWork would add an unfunded allocation and change the canonical wire DTO API"
)]
pub enum NativeCutoverEvidenceSourceFactV1 {
    Allocator {
        next_operation_sequence: u64,
    },
    Reservation {
        target: NativeCutoverEvidenceTargetV1,
        evidence: NativeCutoverReservationEvidenceV1,
        candidate_root_nonce: Option<Digest>,
    },
    Replay {
        workspace_root: String,
        contract_key: String,
        aggregate_identity_digest: String,
        closeout_id: String,
        attempt_id: String,
        format_version: u32,
        state_revision: u64,
        phase: NativeCutoverReplayPhaseV1,
        native_update_request: Option<UpdateRequest>,
        published_snapshot_id: Option<u64>,
        prepared_artifact_digest: Option<NativeCutoverDigest64V1>,
        source_bound_aggregate_receipt_digest: Option<NativeCutoverDigest64V1>,
        terminal_aggregate_receipt_digest: Option<String>,
        terminal_projection_receipt_digest: Option<String>,
        checkpoint_ack: Option<NativeCutoverEvidenceCheckpointAckV1>,
        created_at_epoch_ms: u64,
        updated_at_epoch_ms: u64,
        ordinary_source_binding: Option<NativeCutoverEvidenceSourcePublicationBindingV1>,
    },
    PendingReplay {
        workspace_root: String,
        contract_key: String,
        aggregate_identity_digest: String,
        plan_id: String,
        attempt_id: u32,
        owner_token: u64,
        workspace_identity_digest: NativeCutoverDigest64V1,
        runtime_generation: u64,
        owner_fence_digest: String,
        custody_revision: u64,
    },
    Outbox {
        dispatch_id: String,
        track: String,
        repo_id: String,
        revision_id: String,
        state: String,
        attempt_count: u32,
        publish_binding: Option<NativeCutoverEvidenceOutboxPublishBindingV1>,
        delivery_receipt: Option<NativeCutoverEvidenceOutboxDeliveryReceiptV1>,
        op_count_hint: u64,
        semantic_replace_count_hint: u32,
        semantic_tombstone_count_hint: u32,
    },
    PendingStoreAttempt {
        plan_id: String,
        attempt_id: u32,
        state: String,
        execution_owner_token: Option<u64>,
        fence: NativeCutoverAttemptFenceWireV1,
        started_at_epoch_ms: u64,
        terminal_at_epoch_ms: Option<u64>,
        updated_at_epoch_ms: u64,
    },
    PendingStoreCustody {
        plan_id: String,
        attempt_id: u32,
        custody_revision: u64,
        aggregate_identity_digest: NativeCutoverDigest64V1,
        predecessor_owner_token: u64,
        predecessor_workspace_identity_digest: NativeCutoverDigest64V1,
        predecessor_runtime_generation: u64,
        owner_token: u64,
        owner_workspace_identity_digest: NativeCutoverDigest64V1,
        owner_runtime_generation: u64,
        claimed_at_epoch_ms: u64,
    },
    PendingStoreTerminal {
        plan_id: String,
        attempt_id: u32,
        commit_owner_token: u64,
        canonical_receipt_digest: String,
        publish_store_generation: u64,
        published_snapshot_id: u64,
        checkpoint_generation: Option<u64>,
        checkpoint_payload_digest: Option<NativeCutoverDigest64V1>,
        checkpoint_acked_at_epoch_ms: Option<u64>,
        cleanup_completed_at_epoch_ms: Option<u64>,
        created_at_epoch_ms: u64,
    },
    PendingStoreSdk {
        plan_id: String,
        attempt_id: u32,
        commit_owner_token: u64,
        terminal_receipt_digest: NativeCutoverDigest64V1,
        binding: NativeCutoverEvidenceSdkBindingV1,
        created_at_epoch_ms: u64,
    },
    PendingStoreSdkAck {
        plan_id: String,
        attempt_id: u32,
        commit_owner_token: u64,
        terminal_receipt_digest: NativeCutoverDigest64V1,
        binding: NativeCutoverEvidenceSdkBindingV1,
        canonical_receipt_digest: String,
        acked_binding: Option<NativeCutoverEvidenceSdkBindingV1>,
        checkpoint_generation: u64,
        checkpoint_payload_digest: Digest,
        checkpoint_acked_at_epoch_ms: u64,
        cleanup_completed_at_epoch_ms: Option<u64>,
        created_at_epoch_ms: u64,
    },
    PendingStoreCheckpoint {
        workspace_root: String,
        contract_key: String,
        aggregate_identity_digest: String,
        terminal_state_revision: u64,
        terminal_aggregate_receipt_digest: String,
        ack: NativeCutoverEvidenceCheckpointAckV1,
        created_at_epoch_ms: u64,
    },
    AstFrontier {
        candidate_root_nonce: Option<Digest>,
        workspace: WorkspaceKey,
        workspace_epoch: u64,
        producer: ProducerId,
        stream: StreamId,
        stream_epoch: u64,
        next_operation_sequence: u64,
        committed_event_sequence: u64,
    },
    AstWork {
        candidate_root_nonce: Option<Digest>,
        aggregate_digest: String,
        source_operation: OperationKey,
        lexical_artifact_digest: Digest,
        source_view: ViewRef,
        source_cut: SourceCut,
        revision: u64,
        phase: NativeCutoverAstWorkPhaseV1,
        parser_registration: Option<Digest>,
        ast_operation: Option<OperationKey>,
        ast_event_sequence: Option<u64>,
        claim_owner_epoch: Option<u64>,
        claim_fence: Option<u64>,
        lease_expires_epoch_ms: Option<u64>,
        refusal: Option<Refusal>,
        published_view: Option<ViewRef>,
        superseded_by_aggregate_digest: Option<String>,
        created_at_epoch_ms: u64,
        updated_at_epoch_ms: u64,
        request: Option<UpdateRequest>,
    },
    /// One completed request from the current Source's authenticated ancestry.
    /// Some(Native operation) must join an existing settled Watch V9; None is
    /// a Source-only request and makes no Q publication claim. Event IDs are
    /// a sorted unique membership set, not Host terminal authority.
    CompletedSourceRequest {
        native_operation: Option<OperationKey>,
        source: NativeCutoverEvidenceSourcePublicationBindingV1,
        event_ids: Vec<u128>,
    },
    /// The initial-specific immutable field from one actual settled Native
    /// V9, also matched against the fresh Source's completed ancestry by S.
    /// The pointer snapshot is independent of the frozen FULL snapshot.
    NativeInitialSourceBinding {
        operation: OperationKey,
        aggregate_identity_digest: String,
        source: NativeCutoverEvidenceSourcePublicationBindingV1,
    },
    /// Source ancestry for a settled Native SDK operation. S verifies these
    /// four axes against the actual completed Source request before signing.
    NativeSdkSourceBinding {
        operation: OperationKey,
        aggregate_identity_digest: String,
        source: NativeCutoverEvidenceSourcePublicationBindingV1,
    },
    /// One sibling Source FULL issued from the completed physical Legacy
    /// inventory under the producer's Original and retained FD6 custody.
    ImportedLegacySource {
        head: NativeImportedSourceHeadV1,
        /// Borrowed after SourceJoin checks the genuine reservation fact.
        target: NativeCutoverEvidenceTargetV1,
        /// Borrowed after SourceJoin checks the genuine settled V9 fact.
        request: UpdateRequest,
    },
}
crate::native_serde_enum!(NativeCutoverEvidenceSourceFactV1 {
    units {}
    newtypes {}
    records {
        Allocator = 0 { next_operation_sequence: u64 },
        Reservation = 1 {
            target: NativeCutoverEvidenceTargetV1,
            evidence: NativeCutoverReservationEvidenceV1,
            candidate_root_nonce: Option<Digest>
        },
        Replay = 2 {
            workspace_root: String,
            contract_key: String,
            aggregate_identity_digest: String,
            closeout_id: String,
            attempt_id: String,
            format_version: u32,
            state_revision: u64,
            phase: NativeCutoverReplayPhaseV1,
            native_update_request: Option<UpdateRequest>,
            published_snapshot_id: Option<u64>,
            prepared_artifact_digest: Option<NativeCutoverDigest64V1>,
            source_bound_aggregate_receipt_digest: Option<NativeCutoverDigest64V1>,
            terminal_aggregate_receipt_digest: Option<String>,
            terminal_projection_receipt_digest: Option<String>,
            checkpoint_ack: Option<NativeCutoverEvidenceCheckpointAckV1>,
            created_at_epoch_ms: u64,
            updated_at_epoch_ms: u64,
            ordinary_source_binding: Option<NativeCutoverEvidenceSourcePublicationBindingV1>
        },
        PendingReplay = 3 {
            workspace_root: String,
            contract_key: String,
            aggregate_identity_digest: String,
            plan_id: String,
            attempt_id: u32,
            owner_token: u64,
            workspace_identity_digest: NativeCutoverDigest64V1,
            runtime_generation: u64,
            owner_fence_digest: String,
            custody_revision: u64
        },
        Outbox = 4 {
            dispatch_id: String,
            track: String,
            repo_id: String,
            revision_id: String,
            state: String,
            attempt_count: u32,
            publish_binding: Option<NativeCutoverEvidenceOutboxPublishBindingV1>,
            delivery_receipt: Option<NativeCutoverEvidenceOutboxDeliveryReceiptV1>,
            op_count_hint: u64,
            semantic_replace_count_hint: u32,
            semantic_tombstone_count_hint: u32
        },
        PendingStoreAttempt = 5 {
            plan_id: String,
            attempt_id: u32,
            state: String,
            execution_owner_token: Option<u64>,
            fence: NativeCutoverAttemptFenceWireV1,
            started_at_epoch_ms: u64,
            terminal_at_epoch_ms: Option<u64>,
            updated_at_epoch_ms: u64
        },
        PendingStoreCustody = 6 {
            plan_id: String,
            attempt_id: u32,
            custody_revision: u64,
            aggregate_identity_digest: NativeCutoverDigest64V1,
            predecessor_owner_token: u64,
            predecessor_workspace_identity_digest: NativeCutoverDigest64V1,
            predecessor_runtime_generation: u64,
            owner_token: u64,
            owner_workspace_identity_digest: NativeCutoverDigest64V1,
            owner_runtime_generation: u64,
            claimed_at_epoch_ms: u64
        },
        PendingStoreTerminal = 7 {
            plan_id: String,
            attempt_id: u32,
            commit_owner_token: u64,
            canonical_receipt_digest: String,
            publish_store_generation: u64,
            published_snapshot_id: u64,
            checkpoint_generation: Option<u64>,
            checkpoint_payload_digest: Option<NativeCutoverDigest64V1>,
            checkpoint_acked_at_epoch_ms: Option<u64>,
            cleanup_completed_at_epoch_ms: Option<u64>,
            created_at_epoch_ms: u64
        },
        PendingStoreSdk = 8 {
            plan_id: String,
            attempt_id: u32,
            commit_owner_token: u64,
            terminal_receipt_digest: NativeCutoverDigest64V1,
            binding: NativeCutoverEvidenceSdkBindingV1,
            created_at_epoch_ms: u64
        },
        PendingStoreSdkAck = 9 {
            plan_id: String,
            attempt_id: u32,
            commit_owner_token: u64,
            terminal_receipt_digest: NativeCutoverDigest64V1,
            binding: NativeCutoverEvidenceSdkBindingV1,
            canonical_receipt_digest: String,
            acked_binding: Option<NativeCutoverEvidenceSdkBindingV1>,
            checkpoint_generation: u64,
            checkpoint_payload_digest: Digest,
            checkpoint_acked_at_epoch_ms: u64,
            cleanup_completed_at_epoch_ms: Option<u64>,
            created_at_epoch_ms: u64
        },
        PendingStoreCheckpoint = 10 {
            workspace_root: String,
            contract_key: String,
            aggregate_identity_digest: String,
            terminal_state_revision: u64,
            terminal_aggregate_receipt_digest: String,
            ack: NativeCutoverEvidenceCheckpointAckV1,
            created_at_epoch_ms: u64
        },
        AstFrontier = 11 {
            candidate_root_nonce: Option<Digest>,
            workspace: WorkspaceKey,
            workspace_epoch: u64,
            producer: ProducerId,
            stream: StreamId,
            stream_epoch: u64,
            next_operation_sequence: u64,
            committed_event_sequence: u64
        },
        AstWork = 12 {
            candidate_root_nonce: Option<Digest>,
            aggregate_digest: String,
            source_operation: OperationKey,
            lexical_artifact_digest: Digest,
            source_view: ViewRef,
            source_cut: SourceCut,
            revision: u64,
            phase: NativeCutoverAstWorkPhaseV1,
            parser_registration: Option<Digest>,
            ast_operation: Option<OperationKey>,
            ast_event_sequence: Option<u64>,
            claim_owner_epoch: Option<u64>,
            claim_fence: Option<u64>,
            lease_expires_epoch_ms: Option<u64>,
            refusal: Option<Refusal>,
            published_view: Option<ViewRef>,
            superseded_by_aggregate_digest: Option<String>,
            created_at_epoch_ms: u64,
            updated_at_epoch_ms: u64,
            request: Option<UpdateRequest>
        },
        CompletedSourceRequest = 13 {
            native_operation: Option<OperationKey>,
            source: NativeCutoverEvidenceSourcePublicationBindingV1,
            event_ids: Vec<u128>
        },
        NativeInitialSourceBinding = 14 {
            operation: OperationKey,
            aggregate_identity_digest: String,
            source: NativeCutoverEvidenceSourcePublicationBindingV1
        },
        NativeSdkSourceBinding = 15 {
            operation: OperationKey,
            aggregate_identity_digest: String,
            source: NativeCutoverEvidenceSourcePublicationBindingV1
        },
        ImportedLegacySource = 16 {
            head: NativeImportedSourceHeadV1,
            target: NativeCutoverEvidenceTargetV1,
            request: UpdateRequest
        }
    }
});

impl NativeCutoverEvidenceSourceFactV1 {
    /// Check only the closed Source association shape and selected Source
    /// namespace. Existing reservation, V9, Host and physical joins remain
    /// separate requirements.
    pub fn validate_source_publication_v1(
        &self,
        selected: &NativeCutoverEvidenceSelectedSourceV1,
    ) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        let (operation, source) = match self {
            Self::ImportedLegacySource { head, target, request } => {
                return if head.matches_completed_binding_v1(target, request)
                    && target.operation == head.source_operation.key
                    && request.operation == target.operation {
                    Ok(())
                } else {
                    Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical)
                };
            }
            Self::CompletedSourceRequest { native_operation, source, event_ids } => {
                if event_ids.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1 {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::SequenceTooLarge);
                }
                if event_ids.is_empty() || event_ids.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
                }
                (native_operation.as_ref(), source)
            }
            Self::NativeInitialSourceBinding { operation, aggregate_identity_digest, source }
            | Self::NativeSdkSourceBinding { operation, aggregate_identity_digest, source } => {
                if aggregate_identity_digest.len() != 64
                    || !aggregate_identity_digest.as_bytes().iter()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
                {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
                }
                (Some(operation), source)
            }
            _ => return Ok(()),
        };
        if operation.is_some_and(|operation| {
            operation.workspace != selected.workspace
                || operation.epoch != selected.workspace_epoch
                || operation.producer != selected.producer
                || operation.stream != selected.stream
                || operation.stream_epoch != selected.stream_epoch
        }) || source.target_snapshot_id == 0
            || !canonical_digest64(&source.transition_digest)
            || !canonical_digest64(&source.completion_pointer_digest)
            || !canonical_digest64(&source.physical_target_source_digest)
        {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceSdkBindingV1 {
    pub source_intent_digest: String,
    pub completion_pointer_digest: String,
    pub transition_digest: String,
    pub target_source_digest: String,
    pub target_snapshot: u64,
    pub ingress_kind: String,
    pub workspace_identity_digest: String,
    pub request_fingerprint_digest: String,
    pub client_idempotency_digest: Option<String>,
}
crate::native_serde_struct!(NativeCutoverEvidenceSdkBindingV1 {
    source_intent_digest: String,
    completion_pointer_digest: String,
    transition_digest: String,
    target_source_digest: String,
    target_snapshot: u64,
    ingress_kind: String,
    workspace_identity_digest: String,
    request_fingerprint_digest: String,
    client_idempotency_digest: Option<String>
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceCheckpointAckV1 {
    pub format_version: u32,
    pub checkpoint_id: String,
    pub checkpoint_sequence: u64,
    pub terminal_aggregate_receipt_digest: String,
    pub checkpoint_receipt_digest: String,
    pub canonical_checkpoint_ack_digest: String,
}
crate::native_serde_struct!(NativeCutoverEvidenceCheckpointAckV1 {
    format_version: u32,
    checkpoint_id: String,
    checkpoint_sequence: u64,
    terminal_aggregate_receipt_digest: String,
    checkpoint_receipt_digest: String,
    canonical_checkpoint_ack_digest: String
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidencePreparedAuthorityV1 {
    pub backend: String,
    pub workspace_authority: String,
    pub configured_root: String,
    pub watch_root: String,
    pub mapping_schema: String,
    pub config_seal: String,
}
crate::native_serde_struct!(NativeCutoverEvidencePreparedAuthorityV1 {
    backend: String,
    workspace_authority: String,
    configured_root: String,
    watch_root: String,
    mapping_schema: String,
    config_seal: String
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceOrdinalV1 {
    pub source_ordinal: u64,
    pub admitted: bool,
    pub terminal: Option<String>,
    pub emitted_event_id: Option<u128>,
    pub filtered_no_semantic_work: bool,
    pub reconciliation_event_id: Option<u128>,
    pub source_transition_digest: Option<String>,
    pub target_source_digest: Option<String>,
    pub target_snapshot_id: Option<u64>,
    /// Persisted request identity on a RecoverySubsumed ordinal. Only a
    /// separately revalidated genuine receipt may authorize its use.
    pub recovery_request_digest: Option<String>,
}
crate::native_serde_struct!(NativeCutoverEvidenceOrdinalV1 {
    source_ordinal: u64,
    admitted: bool,
    terminal: Option<String>,
    emitted_event_id: Option<u128>,
    filtered_no_semantic_work: bool,
    reconciliation_event_id: Option<u128>,
    source_transition_digest: Option<String>,
    target_source_digest: Option<String>,
    target_snapshot_id: Option<u64>,
    recovery_request_digest: Option<String>
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceDeliveryV1 {
    pub record_digest: String,
    pub schema_version: String,
    pub sequence: u64,
    pub delivery_id: String,
    pub backend: String,
    pub workspace_authority: String,
    pub configured_root: String,
    pub watch_root: String,
    pub stream_id: String,
    pub prior_cursor: Option<String>,
    pub cursor: String,
    pub payload_digest: String,
    pub mapping_schema: String,
    pub config_seal: String,
    pub input_count: u64,
    pub cursor_receipt_digest: Option<String>,
    pub ordinals: Vec<NativeCutoverEvidenceOrdinalV1>,
}
crate::native_serde_struct!(NativeCutoverEvidenceDeliveryV1 {
    record_digest: String,
    schema_version: String,
    sequence: u64,
    delivery_id: String,
    backend: String,
    workspace_authority: String,
    configured_root: String,
    watch_root: String,
    stream_id: String,
    prior_cursor: Option<String>,
    cursor: String,
    payload_digest: String,
    mapping_schema: String,
    config_seal: String,
    input_count: u64,
    cursor_receipt_digest: Option<String>,
    ordinals: Vec<NativeCutoverEvidenceOrdinalV1>
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidencePendingApplyIntentV1 {
    pub verified_batch_identity: String,
    pub operation_id: String,
    pub workspace_tier: bool,
    pub before_snapshot_id: u64,
    pub after_snapshot_id: u64,
    pub event_ids: Vec<u128>,
}
crate::native_serde_struct!(NativeCutoverEvidencePendingApplyIntentV1 {
    verified_batch_identity: String,
    operation_id: String,
    workspace_tier: bool,
    before_snapshot_id: u64,
    after_snapshot_id: u64,
    event_ids: Vec<u128>
});

/// Scalar projection of one Host-owned full-recovery receipt re-read from
/// the actual checkpoint/publication owner. This is DATA, not a permit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceHostRecoveryReceiptV1 {
    pub operation_id: String,
    pub canonical_workspace_root: String,
    pub request_identity_digest: String,
    pub reason: String,
    pub snapshot_before: u64,
    pub required_snapshot_floor: u64,
    pub failure_revision: Option<u64>,
    pub tail_event_ids: Vec<u128>,
    pub tail_custody_digest: String,
    pub observation_cut_digest: Option<String>,
    pub checkpoint_generation: u64,
    pub checkpoint_payload_digest: String,
    pub published_snapshot_id: u64,
    pub applied_operation_id: String,
    pub applied_request_identity_digest: String,
    pub applied_source_owner_identity: String,
    pub applied_source_execution_authority_digest: String,
    pub applied_source_view_digest: String,
    pub applied_analysis_owner_identity: String,
    pub applied_analysis_lineage_digest: String,
    pub applied_index_owner_identity: String,
    pub applied_index_publication_identity: String,
    pub applied_index_publication_digest: String,
    pub applied_published_snapshot_id: u64,
}
crate::native_serde_struct!(NativeCutoverEvidenceHostRecoveryReceiptV1 {
    operation_id: String,
    canonical_workspace_root: String,
    request_identity_digest: String,
    reason: String,
    snapshot_before: u64,
    required_snapshot_floor: u64,
    failure_revision: Option<u64>,
    tail_event_ids: Vec<u128>,
    tail_custody_digest: String,
    observation_cut_digest: Option<String>,
    checkpoint_generation: u64,
    checkpoint_payload_digest: String,
    published_snapshot_id: u64,
    applied_operation_id: String,
    applied_request_identity_digest: String,
    applied_source_owner_identity: String,
    applied_source_execution_authority_digest: String,
    applied_source_view_digest: String,
    applied_analysis_owner_identity: String,
    applied_analysis_lineage_digest: String,
    applied_index_owner_identity: String,
    applied_index_publication_identity: String,
    applied_index_publication_digest: String,
    applied_published_snapshot_id: u64
});

impl NativeCutoverEvidenceHostRecoveryReceiptV1 {
    /// Recompute the S contract's exact request/tail preimage from this DATA.
    /// The receiver must separately require a genuine Host receipt lender.
    pub fn validate_request_identity_v1(&self) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        if self.operation_id.trim().is_empty()
            || self.canonical_workspace_root.trim().is_empty()
            || self.tail_event_ids.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
            || self.tail_event_ids.windows(2).any(|pair| pair[0] >= pair[1])
            || !matches!(self.reason.as_str(),
                "UntrustedRatioExceeded" | "BatchHintEstimateExceeded" | "QuarantineThreshold"
                | "WorkspaceInvalidation" | "BackpressureBurst" | "WatcherStreamClosed"
                | "WatcherIngestPermanentRejected" | "WatcherIngestUnclassifiedRejected"
                | "WatcherIngestRetryExhausted" | "WatcherSourceReconciliationRequired"
                | "WatcherStartupRegistrationGap" | "WatcherStartupRecovery"
                | "SourceAuthorityRefreshFailed" | "InvalidationIndexUpdateFailed"
                | "InvalidationFactStoreError" | "InvalidationStaleSnapshot"
                | "InvalidationBatchEmpty" | "InvalidationBackendFailed"
                | "InvalidationCheckpointFailed" | "InvalidationAdmissionFailed"
                | "OperatorRequested")
        {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        fn append(hasher: &mut blake3::Hasher, bytes: &[u8]) {
            let _hasher = hasher.update(&(bytes.len() as u64).to_be_bytes()).update(bytes);
        }
        fn raw_digest(text: &str) -> Result<[u8; 32], NativeCutoverEvidenceSyntaxErrorV1> {
            if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
                return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
            }
            let hash = blake3::Hash::from_hex(text)
                .map_err(|_| NativeCutoverEvidenceSyntaxErrorV1::NonCanonical)?;
            Ok(*hash.as_bytes())
        }
        let count = u64::try_from(self.tail_event_ids.len())
            .map_err(|_| NativeCutoverEvidenceSyntaxErrorV1::SequenceTooLarge)?;
        let mut tail = blake3::Hasher::new();
        append(&mut tail, b"quanta.full-rescan-tail-custody.v2");
        append(&mut tail, &count.to_be_bytes());
        for event_id in &self.tail_event_ids {
            append(&mut tail, &event_id.to_be_bytes());
        }
        let tail_digest = *tail.finalize().as_bytes();
        if raw_digest(&self.tail_custody_digest)? != tail_digest {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        let mut request = blake3::Hasher::new();
        for bytes in [
            b"quanta.full-rescan-request-identity.v2".as_slice(),
            self.operation_id.as_bytes(),
            self.canonical_workspace_root.as_bytes(),
            self.reason.as_bytes(),
            &self.snapshot_before.to_be_bytes(),
            &self.required_snapshot_floor.to_be_bytes(),
        ] {
            append(&mut request, bytes);
        }
        match self.failure_revision {
            Some(revision) => { append(&mut request, &[1]); append(&mut request, &revision.to_be_bytes()); }
            None => append(&mut request, &[0]),
        }
        append(&mut request, &count.to_be_bytes());
        append(&mut request, &tail_digest);
        match self.observation_cut_digest.as_deref() {
            Some(cut) => { append(&mut request, &[1]); append(&mut request, &raw_digest(cut)?); }
            None => append(&mut request, &[0]),
        }
        if raw_digest(&self.request_identity_digest)? != *request.finalize().as_bytes() {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceHostFactsV1 {
    pub workspace_authority: String,
    pub capacity: u64,
    pub retained_delivery_count: u64,
    pub active_units: u64,
    pub prepared_authorities: Vec<NativeCutoverEvidencePreparedAuthorityV1>,
    pub deliveries: Vec<NativeCutoverEvidenceDeliveryV1>,
    pub pending_apply_intent: Option<NativeCutoverEvidencePendingApplyIntentV1>,
    pub recovery_receipts: Vec<NativeCutoverEvidenceHostRecoveryReceiptV1>,
}
crate::native_serde_struct!(NativeCutoverEvidenceHostFactsV1 {
    workspace_authority: String,
    capacity: u64,
    retained_delivery_count: u64,
    active_units: u64,
    prepared_authorities: Vec<NativeCutoverEvidencePreparedAuthorityV1>,
    deliveries: Vec<NativeCutoverEvidenceDeliveryV1>,
    pending_apply_intent: Option<NativeCutoverEvidencePendingApplyIntentV1>,
    recovery_receipts: Vec<NativeCutoverEvidenceHostRecoveryReceiptV1>
});

/// An opaque byte string, rather than a JSON-number/CBOR-array projection.
/// The receiver funds decoding and interprets it only with its owning codec.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverByteStringV1(pub Vec<u8>);

impl serde::Serialize for NativeCutoverByteStringV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0.len() > 64 * 1024 {
            return Err(serde::ser::Error::custom(
                "cutover byte string exceeds 64 KiB",
            ));
        }
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for NativeCutoverByteStringV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BytesVisitor;
        impl serde::de::Visitor<'_> for BytesVisitor {
            type Value = NativeCutoverByteStringV1;
            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("a byte string of at most 64 KiB")
            }
            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                if bytes.len() > 64 * 1024 {
                    return Err(E::custom("cutover byte string exceeds 64 KiB"));
                }
                Ok(NativeCutoverByteStringV1(bytes.to_vec()))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
                if bytes.len() > 64 * 1024 {
                    return Err(E::custom("cutover byte string exceeds 64 KiB"));
                }
                Ok(NativeCutoverByteStringV1(bytes))
            }
        }
        deserializer.deserialize_byte_buf(BytesVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceBeforeSelectionV1 {
    pub document: NativeCutoverByteStringV1,
    /// SHA-256 of the retained bytes, distinct from the Q canonical schema2
    /// fingerprint in the request/header's expected-current field.
    pub raw_sha256: Digest,
    /// Physical edition of the first active selection. Older journal records
    /// lack this value and can only reconcile an already selected candidate.
    pub file_version: Option<Digest>,
}
crate::native_serde_struct!(NativeCutoverEvidenceBeforeSelectionV1 {
    document: NativeCutoverByteStringV1,
    raw_sha256: Digest,
    file_version: Option<Digest>
});

impl NativeCutoverEvidenceBeforeSelectionV1 {
    /// A file edition changes when either publisher replaces the path, even
    /// if the replacement contains the same canonical selection document.
    #[cfg(unix)]
    pub fn file_version_v1(metadata: &std::fs::Metadata) -> Digest {
        use sha2::{Digest as _, Sha256};
        use std::os::unix::fs::MetadataExt as _;

        let mut hasher = Sha256::new();
        hasher.update(b"quanta.native-cutover.selection-file-version.v1\0");
        hasher.update(metadata.dev().to_be_bytes());
        hasher.update(metadata.ino().to_be_bytes());
        hasher.update(metadata.len().to_be_bytes());
        hasher.update(metadata.mtime().to_be_bytes());
        hasher.update(metadata.mtime_nsec().to_be_bytes());
        hasher.update(metadata.ctime().to_be_bytes());
        hasher.update(metadata.ctime_nsec().to_be_bytes());
        Digest(hasher.finalize().into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceFreshSdkV1 {
    pub target_snapshot_id: u64,
    pub committed_source_snapshot_id: u64,
    pub committed_source_snapshot_digest: NativeCutoverDigest64V1,
    pub sdk_generation_digest: NativeCutoverDigest64V1,
    pub source_pointer_digest: NativeCutoverDigest64V1,
    pub source_manifest_digest: NativeCutoverDigest64V1,
    pub physical_target_source_digest: NativeCutoverDigest64V1,
}
crate::native_serde_struct!(NativeCutoverEvidenceFreshSdkV1 {
    target_snapshot_id: u64,
    committed_source_snapshot_id: u64,
    committed_source_snapshot_digest: NativeCutoverDigest64V1,
    sdk_generation_digest: NativeCutoverDigest64V1,
    source_pointer_digest: NativeCutoverDigest64V1,
    source_manifest_digest: NativeCutoverDigest64V1,
    physical_target_source_digest: NativeCutoverDigest64V1
});

/// Source's completed cursor covers the previous Native head through the
/// fresh FULL. The receiver binds both endpoints to signed Source and the
/// retained physical Q head before using this assertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
    pub previous_root_nonce: Digest,
    pub previous_source_operation: OperationKey,
    pub previous_source: NativeCutoverEvidenceSourcePublicationBindingV1,
    pub fresh_full_operation: OperationKey,
    pub fresh_source: NativeCutoverEvidenceSourcePublicationBindingV1,
    pub previous_source_generation: u64,
    pub fresh_source_generation: u64,
}
crate::native_serde_struct!(NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
    previous_root_nonce: Digest,
    previous_source_operation: OperationKey,
    previous_source: NativeCutoverEvidenceSourcePublicationBindingV1,
    fresh_full_operation: OperationKey,
    fresh_source: NativeCutoverEvidenceSourcePublicationBindingV1,
    previous_source_generation: u64,
    fresh_source_generation: u64
});

/// Scalar DATA produced only after S exact-reads the referenced V5 artifact
/// with the installed writer and the same Original as the signed V9 census.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceV5ArtifactV1 {
    pub operation: OperationKey,
    pub published_snapshot_id: u64,
    pub lexical_revision_digest: NativeCutoverDigest64V1,
    pub prepared_artifact_digest: NativeCutoverDigest64V1,
    pub aggregate_receipt_digest: NativeCutoverDigest64V1,
    /// The actual predecessor Source transition for a checked DELTA.
    pub previous_transition_digest: Option<NativeCutoverDigest64V1>,
    /// The actual base Source digest for that same checked DELTA.
    pub base_source_digest: Option<NativeCutoverDigest64V1>,
    /// Exact RepoMap custody observed with this V5 artifact. The current
    /// Native receiver independently compares its selected filesystem owner.
    pub repomap: NativeCutoverEvidenceLegacyRepoMapV1,
}
crate::native_serde_struct!(NativeCutoverEvidenceV5ArtifactV1 {
    operation: OperationKey,
    published_snapshot_id: u64,
    lexical_revision_digest: NativeCutoverDigest64V1,
    prepared_artifact_digest: NativeCutoverDigest64V1,
    aggregate_receipt_digest: NativeCutoverDigest64V1,
    previous_transition_digest: Option<NativeCutoverDigest64V1>,
    base_source_digest: Option<NativeCutoverDigest64V1>,
    repomap: NativeCutoverEvidenceLegacyRepoMapV1
});

impl NativeCutoverEvidenceV5ArtifactV1 {
    fn validate_predecessor_pair_v1(&self) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        validate_v5_predecessor_pair_v1(
            self.previous_transition_digest.as_ref(),
            self.base_source_digest.as_ref(),
        )?;
        self.repomap.validate_structure_v1()
    }
}

fn validate_v5_predecessor_pair_v1(
    previous: Option<&NativeCutoverDigest64V1>,
    base: Option<&NativeCutoverDigest64V1>,
) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
    match (previous, base) {
        (None, None) => Ok(()),
        (Some(previous), Some(base))
            if [previous, base].into_iter().all(|digest| {
                digest
                    .0
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
            }) =>
        {
            Ok(())
        }
        _ => Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical),
    }
}

/// Historical Legacy publish and CAS acknowledgement, retained as DATA.
/// Q independently compares current Catalog and deployment state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceLegacyPublicationV1 {
    pub receipt: crate::BatchPublishReceipt,
    pub activation: crate::SearchPlaneSearchCorpusActivationCasAck,
    pub repomap: NativeCutoverEvidenceLegacyRepoMapV1,
    /// Required History publication identified by the Original aggregate.
    pub history: Option<NativeCutoverEvidenceLegacyHistoryV1>,
}

impl serde::Serialize for NativeCutoverEvidenceLegacyPublicationV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        let mut state = serializer.serialize_struct(
            "NativeCutoverEvidenceLegacyPublicationV1",
            3 + usize::from(self.history.is_some()),
        )?;
        state.serialize_field("receipt", &self.receipt)?;
        state.serialize_field("activation", &self.activation)?;
        state.serialize_field("repomap", &self.repomap)?;
        if let Some(history) = &self.history {
            state.serialize_field("history", history)?;
        }
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for NativeCutoverEvidenceLegacyPublicationV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["receipt", "activation", "repomap", "history"];
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = NativeCutoverEvidenceLegacyPublicationV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Legacy publication record")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                use serde::de::Error as _;
                let (mut receipt, mut activation, mut repomap, mut history) =
                    (None, None, None, None);
                macro_rules! read {
                    ($slot:ident) => {{
                        if $slot.is_some() {
                            return Err(A::Error::duplicate_field(stringify!($slot)));
                        }
                        $slot = Some(map.next_value()?);
                    }};
                }
                while let Some(field) =
                    map.next_key_seed(quanta_index_contract_base::ClosedFieldKeySeedV1(FIELDS))?
                {
                    match field {
                        "receipt" => read!(receipt),
                        "activation" => read!(activation),
                        "repomap" => read!(repomap),
                        "history" => read!(history),
                        _ => return Err(A::Error::unknown_field(field, FIELDS)),
                    }
                }
                Ok(NativeCutoverEvidenceLegacyPublicationV1 {
                    receipt: receipt.ok_or_else(|| A::Error::missing_field("receipt"))?,
                    activation: activation.ok_or_else(|| A::Error::missing_field("activation"))?,
                    repomap: repomap.ok_or_else(|| A::Error::missing_field("repomap"))?,
                    history,
                })
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                use serde::de::Error as _;
                let receipt = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::missing_field("receipt"))?;
                let activation = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::missing_field("activation"))?;
                let repomap = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::missing_field("repomap"))?;
                let history = seq.next_element()?;
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(A::Error::invalid_length(5, &self));
                }
                Ok(NativeCutoverEvidenceLegacyPublicationV1 {
                    receipt,
                    activation,
                    repomap,
                    history,
                })
            }
        }
        deserializer.deserialize_struct("NativeCutoverEvidenceLegacyPublicationV1", FIELDS, Visitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceLegacyHistoryV1 {
    pub generation: crate::index::AuxiliaryGenerationKeyV1,
    pub batch_digest: String,
    pub durable_sequence: u64,
}
crate::native_serde_struct!(NativeCutoverEvidenceLegacyHistoryV1 {
    generation: crate::index::AuxiliaryGenerationKeyV1,
    batch_digest: String,
    durable_sequence: u64
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeCutoverEvidenceLegacyRepoMapV1 {
    Required {
        state_root: String,
        locator_digest: NativeCutoverDigest64V1,
        publish: crate::RepoMapTerminalReceiptV2,
        activation: crate::RepoMapTerminalReceiptV2,
    },
    NotApplicable {
        reason_code: String,
        policy_digest: NativeCutoverDigest64V1,
        product_scope_digest: NativeCutoverDigest64V1,
    },
}
crate::native_serde_enum!(NativeCutoverEvidenceLegacyRepoMapV1 {
    units {}
    newtypes {}
    records {
        Required = 0 {
            state_root: String,
            locator_digest: NativeCutoverDigest64V1,
            publish: crate::RepoMapTerminalReceiptV2,
            activation: crate::RepoMapTerminalReceiptV2
        },
        NotApplicable = 1 {
            reason_code: String,
            policy_digest: NativeCutoverDigest64V1,
            product_scope_digest: NativeCutoverDigest64V1
        }
    }
});

impl NativeCutoverEvidenceLegacyRepoMapV1 {
    fn validate_structure_v1(&self) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        let valid = match self {
            Self::Required { state_root, locator_digest, .. } => {
                state_root.starts_with('/')
                    && !state_root.contains('\0')
                    && canonical_digest64(locator_digest)
            }
            Self::NotApplicable { reason_code, policy_digest, product_scope_digest } => {
                !reason_code.is_empty()
                    && canonical_digest64(policy_digest)
                    && canonical_digest64(product_scope_digest)
            }
        };
        valid.then_some(()).ok_or(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical)
    }
}

/// A syntax-complete DATA envelope. This type has no success or permission
/// field; the receiver verifies every fact against its independent owners.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverEvidenceV1 {
    pub header: NativeCutoverEvidenceHeaderV1,
    pub baseline: NativeCutoverEvidenceTargetV1,
    pub before_selection: NativeCutoverEvidenceBeforeSelectionV1,
    pub fresh_sdk: NativeCutoverEvidenceFreshSdkV1,
    pub previous_native_source_coverage: Option<NativeCutoverEvidencePreviousNativeSourceCoverageV1>,
    pub v5_artifacts: Vec<NativeCutoverEvidenceV5ArtifactV1>,
    pub legacy_publication: Option<NativeCutoverEvidenceLegacyPublicationV1>,
    /// Retained terminal DATA for independent Q reconciliation. A completed
    /// archive supplies its latest durable completion while preserving the
    /// immutable first terminal separately. These bytes grant no authority.
    pub retained_terminal: Option<NativeCutoverByteStringV1>,
    pub source_facts: Vec<NativeCutoverEvidenceSourceFactV1>,
    pub host_facts: NativeCutoverEvidenceHostFactsV1,
}
crate::native_serde_struct!(NativeCutoverEvidenceV1 {
    header: NativeCutoverEvidenceHeaderV1,
    baseline: NativeCutoverEvidenceTargetV1,
    before_selection: NativeCutoverEvidenceBeforeSelectionV1,
    fresh_sdk: NativeCutoverEvidenceFreshSdkV1,
    previous_native_source_coverage: Option<NativeCutoverEvidencePreviousNativeSourceCoverageV1>,
    v5_artifacts: Vec<NativeCutoverEvidenceV5ArtifactV1>,
    legacy_publication: Option<NativeCutoverEvidenceLegacyPublicationV1>,
    retained_terminal: Option<NativeCutoverByteStringV1>,
    source_facts: Vec<NativeCutoverEvidenceSourceFactV1>,
    host_facts: NativeCutoverEvidenceHostFactsV1
});

impl NativeCutoverEvidenceV1 {
    pub const FORMAT_VERSION_V1: u32 = 5;

    /// Structural bounds checked after syntax admission and typed decode.
    /// This is deliberately not a quiescence or publication predicate.
    pub fn validate_structure_v1(&self) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        if self.header.format_version != Self::FORMAT_VERSION_V1 {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::Unsupported);
        }
        if self.source_facts.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
            || self.v5_artifacts.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
            || self.host_facts.deliveries.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
            || self.host_facts.prepared_authorities.len()
                > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
            || self.host_facts.recovery_receipts.len()
                > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
        {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::SequenceTooLarge);
        }
        for artifact in &self.v5_artifacts {
            artifact.validate_predecessor_pair_v1()?;
        }
        if self.host_facts.recovery_receipts.iter().any(|receipt| {
            receipt.tail_event_ids.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
        }) {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::SequenceTooLarge);
        }
        for fact in &self.source_facts {
            if let NativeCutoverEvidenceSourceFactV1::NativeInitialSourceBinding { operation, .. } = fact
                && operation.workspace != self.header.selected_source.workspace
            {
                let imported = self.source_facts.iter().any(|other| matches!(other,
                    NativeCutoverEvidenceSourceFactV1::ImportedLegacySource { head, .. }
                        if head.source_operation.key == *operation));
                if !imported {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
                }
                fact.validate_source_publication_v1(&NativeCutoverEvidenceSelectedSourceV1 {
                    workspace: operation.workspace,
                    workspace_epoch: operation.epoch,
                    producer: operation.producer,
                    stream: operation.stream,
                    stream_epoch: operation.stream_epoch,
                    text_admission: self.header.selected_source.text_admission,
                })?;
            } else {
                fact.validate_source_publication_v1(&self.header.selected_source)?;
            }
        }
        if let Some(legacy) = &self.legacy_publication {
            legacy.repomap.validate_structure_v1()?;
            if legacy.history.as_ref().is_some_and(|history| {
                history.durable_sequence == 0
                    || !crate::is_canonical_batch_digest_token_v1(&history.batch_digest)
            }) {
                return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
            }
        }
        if self.before_selection.document.0.is_empty()
            || self.before_selection.document.0.len() > 64 * 1024
            || self.retained_terminal.as_ref().is_some_and(|wire| {
                wire.0.is_empty() || wire.0.len() > MAX_NATIVE_CUTOVER_CONTROL_BYTES_V1
            })
        {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::TooLarge);
        }
        let delivery_count = u64::try_from(self.host_facts.deliveries.len())
            .map_err(|_error| NativeCutoverEvidenceSyntaxErrorV1::SequenceTooLarge)?;
        if self.host_facts.retained_delivery_count != delivery_count {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        if self.header.original_full_operation != self.baseline.operation
            || self.header.canonical_workspace_root != self.header.host.canonical_workspace_root
            || self.header.host.custody_mode != NativeCutoverHostCustodyModeV1::DurableInbox
            || self.header.writer_state_root.is_empty()
            || self.header.canonical_snapshot_namespace.is_empty()
            || self.header.sdk_lock_path.is_empty()
            || self.header.config_path.is_empty()
        {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        Ok(())
    }
}

fn canonical_digest64(digest: &NativeCutoverDigest64V1) -> bool {
    digest
        .0
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCutoverRecoveryModeV1 {
    RollbackLegacy,
    RestoreForward,
}
crate::native_serde_enum!(NativeCutoverRecoveryModeV1 {
    units { RollbackLegacy = 0, RestoreForward = 1 }
    newtypes {}
    records {}
});

/// FD3 request header. It selects a recovery mode but grants no publication
/// authority; the Q receiver still verifies its own BOOT, root lease and CAS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverControlBeginV1 {
    pub format_version: u32,
    pub operation_id: String,
    pub mode: NativeCutoverRecoveryModeV1,
    pub current_selection_sha256: Digest,
    pub candidate_selection_sha256: Digest,
    pub deadline_ns: u64,
    pub evidence_length: u64,
    pub evidence_sha256: Digest,
    pub nonce: Digest,
}
crate::native_serde_struct!(NativeCutoverControlBeginV1 {
    format_version: u32,
    operation_id: String,
    mode: NativeCutoverRecoveryModeV1,
    current_selection_sha256: Digest,
    candidate_selection_sha256: Digest,
    deadline_ns: u64,
    evidence_length: u64,
    evidence_sha256: Digest,
    nonce: Digest
});

impl NativeCutoverControlBeginV1 {
    pub const FORMAT_VERSION_V1: u32 = 1;

    pub fn validate_v1(&self) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        let maximum_evidence_length = u64::try_from(MAX_NATIVE_CUTOVER_EVIDENCE_BYTES_V1)
            .map_err(|_error| NativeCutoverEvidenceSyntaxErrorV1::TooLarge)?;
        if self.format_version != Self::FORMAT_VERSION_V1
            || self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || self.evidence_length == 0
            || self.deadline_ns == 0
            || self.evidence_length > maximum_evidence_length
        {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverControlCancelV1 {
    pub format_version: u32,
    pub operation_id: String,
    pub nonce: Digest,
}
crate::native_serde_struct!(NativeCutoverControlCancelV1 {
    format_version: u32,
    operation_id: String,
    nonce: Digest
});

impl NativeCutoverControlCancelV1 {
    pub fn validate_v1(&self) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        if self.format_version != NativeCutoverControlBeginV1::FORMAT_VERSION_V1
            || self.operation_id.is_empty()
            || self.operation_id.len() > 128
        {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        Ok(())
    }
}

/// The terminal History FULL selected by installed BOOT. Earlier mounted
/// generations remain in the same verified Catalog closure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCutoverInstalledHistoryV1 {
    pub operation: OperationKey,
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub member: ObjectRef,
    pub source_input: ObjectRef,
    pub cut: HistorySourceCutV1,
}
crate::native_serde_struct!(NativeCutoverInstalledHistoryV1 {
    operation: OperationKey,
    view: ViewRef,
    instance: IndexInstanceId,
    member: ObjectRef,
    source_input: ObjectRef,
    cut: HistorySourceCutV1
});

impl NativeCutoverInstalledHistoryV1 {
    pub fn validate_v1(&self) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        if self.operation.workspace != self.view.workspace
            || self.operation.epoch != self.view.epoch
            || self.operation.sequence == 0
            || self.operation.stream_epoch == 0
            || self.view.sequence == 0
            || self.view.digest == Digest([0; 32])
            || self.member.0 == Digest([0; 32])
            || self.source_input.0 == Digest([0; 32])
            || self.cut.durable_epoch == crate::AuxEpochV1::GENESIS
            || self.cut.snapshot_digest == Digest([0; 32])
            || self.cut.replay_start == Digest([0; 32])
        {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeCutoverControlTerminalV1 {
    Applied {
        operation_id: String,
        nonce: Digest,
        before_sha256: Digest,
        after_sha256: Digest,
    },
    /// The requested current candidate was independently revalidated and
    /// synced. This does not attribute a historical rename to this operation.
    Reconciled {
        operation_id: String,
        nonce: Digest,
        before_sha256: Digest,
        after_sha256: Digest,
    },
    AppliedHistory {
        operation_id: String,
        nonce: Digest,
        before_sha256: Digest,
        after_sha256: Digest,
        history: NativeCutoverInstalledHistoryV1,
    },
    ReconciledHistory {
        operation_id: String,
        nonce: Digest,
        before_sha256: Digest,
        after_sha256: Digest,
        history: NativeCutoverInstalledHistoryV1,
    },
    RefusedBeforeRename {
        operation_id: String,
        nonce: Digest,
        code: String,
        message: String,
    },
    PersistenceUncertain {
        operation_id: String,
        nonce: Digest,
        before_sha256: Digest,
        after_sha256: Option<Digest>,
        code: String,
        message: String,
    },
}
crate::native_serde_enum!(NativeCutoverControlTerminalV1 {
    units {}
    newtypes {}
    records {
        Applied = 0 {
            operation_id: String,
            nonce: Digest,
            before_sha256: Digest,
            after_sha256: Digest
        },
        Reconciled = 3 {
            operation_id: String,
            nonce: Digest,
            before_sha256: Digest,
            after_sha256: Digest
        },
        AppliedHistory = 4 {
            operation_id: String,
            nonce: Digest,
            before_sha256: Digest,
            after_sha256: Digest,
            history: NativeCutoverInstalledHistoryV1
        },
        ReconciledHistory = 5 {
            operation_id: String,
            nonce: Digest,
            before_sha256: Digest,
            after_sha256: Digest,
            history: NativeCutoverInstalledHistoryV1
        },
        RefusedBeforeRename = 1 {
            operation_id: String,
            nonce: Digest,
            code: String,
            message: String
        },
        PersistenceUncertain = 2 {
            operation_id: String,
            nonce: Digest,
            before_sha256: Digest,
            after_sha256: Option<Digest>,
            code: String,
            message: String
        }
    }
});

impl NativeCutoverControlTerminalV1 {
    #[must_use]
    pub const fn installed_history_v1(&self) -> Option<&NativeCutoverInstalledHistoryV1> {
        match self {
            Self::AppliedHistory { history, .. } | Self::ReconciledHistory { history, .. } => Some(history),
            _ => None,
        }
    }

    pub fn validate_v1(&self) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        let (operation_id, code, message) = match self {
            Self::Applied { operation_id, .. }
            | Self::Reconciled { operation_id, .. }
            | Self::AppliedHistory { operation_id, .. }
            | Self::ReconciledHistory { operation_id, .. } => {
                (operation_id.as_str(), None, None)
            }
            Self::RefusedBeforeRename {
                operation_id,
                code,
                message,
                ..
            }
            | Self::PersistenceUncertain {
                operation_id,
                code,
                message,
                ..
            } => (
                operation_id.as_str(),
                Some(code.as_str()),
                Some(message.as_str()),
            ),
        };
        if operation_id.is_empty()
            || operation_id.len() > 128
            || code.is_some_and(|value| value.is_empty() || value.len() > 128)
            || message.is_some_and(|value| value.len() > 4096)
        {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        if let Some(history) = self.installed_history_v1() {
            history.validate_v1()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCutoverEvidenceSyntaxErrorV1 {
    Empty,
    TooLarge,
    Truncated,
    NonCanonical,
    Unsupported,
    TextTooLarge,
    InvalidUtf8,
    SequenceTooLarge,
    MapTooLarge,
    DuplicateField,
    TooDeep,
    TrailingData,
}

impl core::fmt::Display for NativeCutoverEvidenceSyntaxErrorV1 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "invalid Native cutover evidence CBOR: {self:?}")
    }
}

impl std::error::Error for NativeCutoverEvidenceSyntaxErrorV1 {}

#[derive(Debug)]
pub enum NativeCutoverEvidenceDecodeErrorV1 {
    Syntax(NativeCutoverEvidenceSyntaxErrorV1),
    Cbor(ciborium::de::Error<std::io::Error>),
    Structure(NativeCutoverEvidenceSyntaxErrorV1),
}

impl core::fmt::Display for NativeCutoverEvidenceDecodeErrorV1 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Syntax(error) | Self::Structure(error) => error.fmt(formatter),
            Self::Cbor(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for NativeCutoverEvidenceDecodeErrorV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Syntax(error) | Self::Structure(error) => Some(error),
            Self::Cbor(error) => Some(error),
        }
    }
}

struct SyntaxReaderV1<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl SyntaxReaderV1<'_> {
    fn take_v1(&mut self, count: usize) -> Result<&[u8], NativeCutoverEvidenceSyntaxErrorV1> {
        let end = self
            .cursor
            .checked_add(count)
            .ok_or(NativeCutoverEvidenceSyntaxErrorV1::Truncated)?;
        let slice = self
            .bytes
            .get(self.cursor..end)
            .ok_or(NativeCutoverEvidenceSyntaxErrorV1::Truncated)?;
        self.cursor = end;
        Ok(slice)
    }

    fn head_v1(&mut self) -> Result<(u8, u8), NativeCutoverEvidenceSyntaxErrorV1> {
        let byte = *self
            .take_v1(1)?
            .first()
            .ok_or(NativeCutoverEvidenceSyntaxErrorV1::Truncated)?;
        Ok((byte >> 5, byte & 31))
    }

    fn argument_v1(&mut self, extra: u8) -> Result<u64, NativeCutoverEvidenceSyntaxErrorV1> {
        let (count, minimum) = match extra {
            0..=23 => return Ok(u64::from(extra)),
            24 => (1, 24),
            25 => (2, 256),
            26 => (4, 65_536),
            27 => (8, 4_294_967_296),
            _ => return Err(NativeCutoverEvidenceSyntaxErrorV1::Unsupported),
        };
        let mut value = 0_u64;
        for byte in self.take_v1(count)? {
            value = (value << 8) | u64::from(*byte);
        }
        if value < minimum {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
        }
        Ok(value)
    }

    fn length_v1(&mut self, extra: u8) -> Result<usize, NativeCutoverEvidenceSyntaxErrorV1> {
        usize::try_from(self.argument_v1(extra)?)
            .map_err(|_error| NativeCutoverEvidenceSyntaxErrorV1::TooLarge)
    }

    fn text_v1(&mut self, extra: u8) -> Result<(usize, usize), NativeCutoverEvidenceSyntaxErrorV1> {
        let length = self.length_v1(extra)?;
        if length > MAX_NATIVE_CUTOVER_EVIDENCE_TEXT_BYTES_V1 {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::TextTooLarge);
        }
        let start = self.cursor;
        let text = self.take_v1(length)?;
        let _validated_text = core::str::from_utf8(text)
            .map_err(|_error| NativeCutoverEvidenceSyntaxErrorV1::InvalidUtf8)?;
        Ok((start, self.cursor))
    }

    fn item_v1(&mut self, depth: usize) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
        if depth > MAX_DEPTH_V1 {
            return Err(NativeCutoverEvidenceSyntaxErrorV1::TooDeep);
        }
        let (major, extra) = self.head_v1()?;
        match major {
            0 | 1 => {
                let _scalar = self.argument_v1(extra)?;
            }
            2 => {
                let length = self.length_v1(extra)?;
                if length > MAX_NATIVE_CUTOVER_EVIDENCE_BYTES_V1 {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::TooLarge);
                }
                let _bytes = self.take_v1(length)?;
            }
            3 => {
                let _text = self.text_v1(extra)?;
            }
            4 => {
                let count = self.length_v1(extra)?;
                if count > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1 {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::SequenceTooLarge);
                }
                let nested_depth = depth
                    .checked_add(1)
                    .ok_or(NativeCutoverEvidenceSyntaxErrorV1::TooDeep)?;
                for _ in 0..count {
                    self.item_v1(nested_depth)?;
                }
            }
            5 => {
                let count = self.length_v1(extra)?;
                if count > MAX_NATIVE_CUTOVER_EVIDENCE_MAP_FIELDS_V1 {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::MapTooLarge);
                }
                let mut keys = [(0_usize, 0_usize); MAX_NATIVE_CUTOVER_EVIDENCE_MAP_FIELDS_V1];
                for previous in 0..count {
                    let (key_major, key_extra) = self.head_v1()?;
                    if key_major != 3 {
                        return Err(NativeCutoverEvidenceSyntaxErrorV1::Unsupported);
                    }
                    let (start, end) = self.text_v1(key_extra)?;
                    let key = self
                        .bytes
                        .get(start..end)
                        .ok_or(NativeCutoverEvidenceSyntaxErrorV1::Truncated)?;
                    for (old_start, old_end) in keys
                        .get(..previous)
                        .ok_or(NativeCutoverEvidenceSyntaxErrorV1::MapTooLarge)?
                        .iter()
                        .copied()
                    {
                        if self
                            .bytes
                            .get(old_start..old_end)
                            .ok_or(NativeCutoverEvidenceSyntaxErrorV1::Truncated)?
                            == key
                        {
                            return Err(NativeCutoverEvidenceSyntaxErrorV1::DuplicateField);
                        }
                    }
                    *keys
                        .get_mut(previous)
                        .ok_or(NativeCutoverEvidenceSyntaxErrorV1::MapTooLarge)? = (start, end);
                    let nested_depth = depth
                        .checked_add(1)
                        .ok_or(NativeCutoverEvidenceSyntaxErrorV1::TooDeep)?;
                    self.item_v1(nested_depth)?;
                }
            }
            6 => {
                // Ciborium encodes Host u128 event IDs above u64::MAX as
                // RFC 8949 positive bignums. No other tagged value belongs
                // to this closed DATA protocol.
                if self.argument_v1(extra)? != 2 {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::Unsupported);
                }
                let (payload_major, payload_extra) = self.head_v1()?;
                if payload_major != 2 {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::Unsupported);
                }
                let length = self.length_v1(payload_extra)?;
                if !(9..=16).contains(&length) {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
                }
                if self.take_v1(length)?.first() == Some(&0) {
                    return Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical);
                }
            }
            7 if matches!(extra, 20..=22) => {}
            _ => return Err(NativeCutoverEvidenceSyntaxErrorV1::Unsupported),
        }
        Ok(())
    }
}

/// Allocation-free, whole-buffer preflight. Call before `ciborium` decode;
/// schema-specific Deserialize then rejects unknown fields and variants.
pub fn validate_native_cutover_evidence_cbor_v1(
    bytes: &[u8],
) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
    validate_cutover_cbor_with_limit_v1(bytes, MAX_NATIVE_CUTOVER_EVIDENCE_BYTES_V1)
}

fn validate_cutover_cbor_with_limit_v1(
    bytes: &[u8],
    max_bytes: usize,
) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
    if bytes.is_empty() {
        return Err(NativeCutoverEvidenceSyntaxErrorV1::Empty);
    }
    if bytes.len() > max_bytes {
        return Err(NativeCutoverEvidenceSyntaxErrorV1::TooLarge);
    }
    let mut reader = SyntaxReaderV1 { bytes, cursor: 0 };
    reader.item_v1(0)?;
    if reader.cursor != bytes.len() {
        return Err(NativeCutoverEvidenceSyntaxErrorV1::TrailingData);
    }
    Ok(())
}

pub fn validate_native_cutover_control_cbor_v1(
    bytes: &[u8],
) -> Result<(), NativeCutoverEvidenceSyntaxErrorV1> {
    validate_cutover_cbor_with_limit_v1(bytes, MAX_NATIVE_CUTOVER_CONTROL_BYTES_V1)
}

pub fn decode_native_cutover_control_begin_v1(
    bytes: &[u8],
) -> Result<NativeCutoverControlBeginV1, NativeCutoverEvidenceDecodeErrorV1> {
    validate_native_cutover_control_cbor_v1(bytes)
        .map_err(NativeCutoverEvidenceDecodeErrorV1::Syntax)?;
    let decoded: NativeCutoverControlBeginV1 =
        ciborium::de::from_reader(std::io::Cursor::new(bytes))
            .map_err(NativeCutoverEvidenceDecodeErrorV1::Cbor)?;
    decoded
        .validate_v1()
        .map_err(NativeCutoverEvidenceDecodeErrorV1::Structure)?;
    Ok(decoded)
}

pub fn decode_native_cutover_control_cancel_v1(
    bytes: &[u8],
) -> Result<NativeCutoverControlCancelV1, NativeCutoverEvidenceDecodeErrorV1> {
    validate_native_cutover_control_cbor_v1(bytes)
        .map_err(NativeCutoverEvidenceDecodeErrorV1::Syntax)?;
    let decoded: NativeCutoverControlCancelV1 =
        ciborium::de::from_reader(std::io::Cursor::new(bytes))
            .map_err(NativeCutoverEvidenceDecodeErrorV1::Cbor)?;
    decoded
        .validate_v1()
        .map_err(NativeCutoverEvidenceDecodeErrorV1::Structure)?;
    Ok(decoded)
}

pub fn decode_native_cutover_control_terminal_v1(
    bytes: &[u8],
) -> Result<NativeCutoverControlTerminalV1, NativeCutoverEvidenceDecodeErrorV1> {
    validate_native_cutover_control_cbor_v1(bytes)
        .map_err(NativeCutoverEvidenceDecodeErrorV1::Syntax)?;
    let decoded: NativeCutoverControlTerminalV1 =
        ciborium::de::from_reader(std::io::Cursor::new(bytes))
            .map_err(NativeCutoverEvidenceDecodeErrorV1::Cbor)?;
    decoded
        .validate_v1()
        .map_err(NativeCutoverEvidenceDecodeErrorV1::Structure)?;
    Ok(decoded)
}

/// Decode only after whole-file physical bound and syntax admission. The
/// closed manual Serde types reject unknown fields, variants, and duplicates.
pub fn decode_native_cutover_evidence_v1(
    bytes: &[u8],
) -> Result<NativeCutoverEvidenceV1, NativeCutoverEvidenceDecodeErrorV1> {
    validate_native_cutover_evidence_cbor_v1(bytes)
        .map_err(NativeCutoverEvidenceDecodeErrorV1::Syntax)?;
    let decoded: NativeCutoverEvidenceV1 = ciborium::de::from_reader(std::io::Cursor::new(bytes))
        .map_err(NativeCutoverEvidenceDecodeErrorV1::Cbor)?;
    decoded
        .validate_structure_v1()
        .map_err(NativeCutoverEvidenceDecodeErrorV1::Structure)?;
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed_watch_v1() -> NativeCutoverEvidenceSourceFactV1 {
        use crate::index::{CollectionId, SlotId, TenantId, WorktreeId};
        NativeCutoverEvidenceSourceFactV1::CompletedSourceRequest {
            native_operation: Some(OperationKey {
                workspace: WorkspaceKey { tenant: TenantId([1; 16]), collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]), slot: SlotId([4; 16]) },
                epoch: 7, producer: ProducerId([5; 16]), stream: StreamId([6; 16]),
                stream_epoch: 8, sequence: 9,
            }),
            source: NativeCutoverEvidenceSourcePublicationBindingV1 {
                transition_digest: NativeCutoverDigest64V1([b'a'; 64]),
                completion_pointer_digest: NativeCutoverDigest64V1([b'b'; 64]),
                physical_target_source_digest: NativeCutoverDigest64V1([b'c'; 64]),
                target_snapshot_id: 42,
            },
            event_ids: vec![0, u128::from(u64::MAX), 1_u128 << 64, 1_u128 << 127, u128::MAX],
        }
    }

    fn watch_scope_v1() -> NativeCutoverEvidenceSelectedSourceV1 {
        let NativeCutoverEvidenceSourceFactV1::CompletedSourceRequest { native_operation: Some(operation), .. } = completed_watch_v1()
            else { unreachable!() };
        NativeCutoverEvidenceSelectedSourceV1 {
            workspace: operation.workspace, workspace_epoch: operation.epoch,
            producer: operation.producer, stream: operation.stream, stream_epoch: operation.stream_epoch,
            text_admission: SourceTextAdmission::Excluded,
        }
    }

    fn binding_golden_v1() -> ciborium::value::Value {
        use ciborium::value::Value as V;
        // Digest64 uses the existing fixed 64-element tuple wire, rather than
        // a CBOR byte string. Keep that original representation unchanged.
        let digest_v1 = |byte: u8| V::Array(vec![V::Integer(byte.into()); 64]);
        V::Map(vec![
            (V::Text("transition_digest".into()), digest_v1(b'a')),
            (V::Text("completion_pointer_digest".into()), digest_v1(b'b')),
            (V::Text("physical_target_source_digest".into()), digest_v1(b'c')),
            (V::Text("target_snapshot_id".into()), V::Integer(42.into())),
        ])
    }

    fn completed_watch_golden_v1() -> ciborium::value::Value {
        use ciborium::value::Value as V;
        let id_v1 = |byte: u8| V::Array(vec![V::Integer(byte.into()); 16]);
        let workspace_v1 = V::Map(vec![
            (V::Text("tenant".into()), id_v1(1)), (V::Text("collection".into()), id_v1(2)),
            (V::Text("worktree".into()), id_v1(3)), (V::Text("slot".into()), id_v1(4)),
        ]);
        let operation_v1 = V::Map(vec![
            (V::Text("workspace".into()), workspace_v1), (V::Text("epoch".into()), V::Integer(7.into())),
            (V::Text("producer".into()), id_v1(5)), (V::Text("stream".into()), id_v1(6)),
            (V::Text("stream_epoch".into()), V::Integer(8.into())), (V::Text("sequence".into()), V::Integer(9.into())),
        ]);
        let mut first_bignum_v1 = vec![0; 9]; first_bignum_v1[0] = 1;
        let mut high_bignum_v1 = vec![0; 16]; high_bignum_v1[0] = 0x80;
        V::Map(vec![(V::Text("CompletedSourceRequest".into()), V::Map(vec![
            (V::Text("native_operation".into()), operation_v1),
            (V::Text("source".into()), binding_golden_v1()),
            (V::Text("event_ids".into()), V::Array(vec![
                V::Integer(0.into()), V::Integer(u64::MAX.into()),
                V::Tag(2, Box::new(V::Bytes(first_bignum_v1))),
                V::Tag(2, Box::new(V::Bytes(high_bignum_v1))),
                V::Tag(2, Box::new(V::Bytes(vec![0xff; 16]))),
            ])),
        ]))])
    }

    fn cbor_v1(value: &impl serde::Serialize) -> Vec<u8> {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(value, &mut bytes).expect("CBOR fixture encode");
        bytes
    }

    #[test]
    fn source_publication_binding_preserves_original_four_field_wire_v1() {
        let NativeCutoverEvidenceSourceFactV1::CompletedSourceRequest { source, .. } = completed_watch_v1()
            else { unreachable!() };
        assert_eq!(cbor_v1(&source), cbor_v1(&binding_golden_v1()));
    }

    fn initial_source_fact_v1() -> NativeCutoverEvidenceSourceFactV1 {
        let NativeCutoverEvidenceSourceFactV1::CompletedSourceRequest { native_operation: Some(operation), source, .. } = completed_watch_v1()
            else { unreachable!() };
        NativeCutoverEvidenceSourceFactV1::NativeInitialSourceBinding {
            operation, aggregate_identity_digest: "d".repeat(64), source,
        }
    }

    fn initial_source_golden_v1() -> ciborium::value::Value {
        use ciborium::value::Value as V;
        // Reuse only the independent literal OperationKey and Source4 golden.
        // No encoded implementation value contributes to this expected wire.
        let V::Map(mut variants) = completed_watch_golden_v1() else { unreachable!() };
        let (_, V::Map(mut fields)) = variants.pop().expect("one literal variant") else { unreachable!() };
        drop(fields.pop());
        fields[0].0 = V::Text("operation".into());
        fields.insert(1, (V::Text("aggregate_identity_digest".into()), V::Text("d".repeat(64))));
        V::Map(vec![(V::Text("NativeInitialSourceBinding".into()), V::Map(fields))])
    }

    #[test]
    fn native_initial_source_wire_has_one_distinct_closed_record_v1() {
        let fact = initial_source_fact_v1();
        let bytes = cbor_v1(&initial_source_golden_v1());
        assert_eq!(cbor_v1(&fact), bytes);
        assert_eq!(ciborium::de::from_reader::<NativeCutoverEvidenceSourceFactV1, _>(bytes.as_slice())
            .expect("independent initial record wire"), fact);
        for mutation in 0..3 {
            use ciborium::value::Value as V;
            let V::Map(mut variants) = initial_source_golden_v1() else { unreachable!() };
            let V::Map(fields) = &mut variants[0].1 else { unreachable!() };
            match mutation {
                0 => { drop(fields.pop()); }
                1 => fields.push((V::Text("permission".into()), V::Bool(true))),
                _ => fields.push(fields[1].clone()),
            }
            let malformed = cbor_v1(&V::Map(variants));
            assert!(ciborium::de::from_reader::<NativeCutoverEvidenceSourceFactV1, _>(malformed.as_slice()).is_err());
        }
    }

    #[test]
    fn native_sdk_source_has_distinct_closed_wire_and_no_compat_alias_v1() {
        use ciborium::value::Value as V;
        let NativeCutoverEvidenceSourceFactV1::NativeInitialSourceBinding {
            operation, aggregate_identity_digest, source,
        } = initial_source_fact_v1() else { unreachable!() };
        let fact = NativeCutoverEvidenceSourceFactV1::NativeSdkSourceBinding {
            operation, aggregate_identity_digest, source,
        };
        let V::Map(mut variants) = initial_source_golden_v1() else { unreachable!() };
        variants[0].0 = V::Text("NativeSdkSourceBinding".into());
        let golden = V::Map(variants);
        assert_eq!(cbor_v1(&fact), cbor_v1(&golden));
        assert_eq!(ciborium::de::from_reader::<NativeCutoverEvidenceSourceFactV1, _>(
            cbor_v1(&golden).as_slice()).unwrap(), fact);
        let mut malformed = golden;
        let V::Map(variants) = &mut malformed else { unreachable!() };
        let V::Map(fields) = &mut variants[0].1 else { unreachable!() };
        drop(fields.pop());
        assert!(ciborium::de::from_reader::<NativeCutoverEvidenceSourceFactV1, _>(
            cbor_v1(&malformed).as_slice()).is_err());
    }

    #[test]
    fn native_initial_source_shape_refuses_foreign_or_partial_axes_v1() {
        let selected = watch_scope_v1();
        assert_eq!(initial_source_fact_v1().validate_source_publication_v1(&selected), Ok(()));
        for mutation in 0..6 {
            let mut fact = initial_source_fact_v1();
            let NativeCutoverEvidenceSourceFactV1::NativeInitialSourceBinding { operation, aggregate_identity_digest, source } = &mut fact
                else { unreachable!() };
            match mutation {
                0 => operation.epoch += 1,
                1 => aggregate_identity_digest.clear(),
                2 => *aggregate_identity_digest = "D".repeat(64),
                3 => source.target_snapshot_id = 0,
                4 => source.transition_digest.0[0] = b'g',
                _ => source.completion_pointer_digest.0[0] = b'G',
            }
            assert!(fact.validate_source_publication_v1(&selected).is_err());
        }
    }

    #[test]
    fn completed_watch_wire_matches_independent_golden_and_full_u128_v1() {
        let expected = completed_watch_v1();
        let bytes = cbor_v1(&completed_watch_golden_v1());
        assert_eq!(cbor_v1(&expected), bytes);
        assert_eq!(validate_native_cutover_evidence_cbor_v1(&bytes), Ok(()));
        let actual: NativeCutoverEvidenceSourceFactV1 = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.validate_source_publication_v1(&watch_scope_v1()), Ok(()));
    }

    #[test]
    fn source_only_completed_request_wire_has_null_native_operation_v1() {
        use ciborium::value::Value as V;
        let mut fact = completed_watch_v1();
        let NativeCutoverEvidenceSourceFactV1::CompletedSourceRequest { native_operation, .. } = &mut fact
            else { unreachable!() };
        *native_operation = None;
        let mut golden = completed_watch_golden_v1();
        let V::Map(variants) = &mut golden else { unreachable!() };
        let V::Map(fields) = &mut variants[0].1 else { unreachable!() };
        fields[0].1 = V::Null;
        assert_eq!(cbor_v1(&fact), cbor_v1(&golden));
        assert_eq!(ciborium::de::from_reader::<NativeCutoverEvidenceSourceFactV1, _>(
            cbor_v1(&golden).as_slice()).unwrap(), fact);
        assert_eq!(fact.validate_source_publication_v1(&watch_scope_v1()), Ok(()));
    }

    #[test]
    fn host_recovery_request_identity_matches_independent_s_contract_golden_v1() {
        let receipt = NativeCutoverEvidenceHostRecoveryReceiptV1 {
            operation_id: "op-1".into(), canonical_workspace_root: "/repo".into(),
            request_identity_digest: "c9682b0f4169c569c2af8d707c53fd3f53a7db7fcfe1688941131c22d6282c5c".into(),
            reason: "WatcherStartupRecovery".into(), snapshot_before: 4, required_snapshot_floor: 5,
            failure_revision: Some(17),
            tail_event_ids: vec![u128::from_be_bytes([0x0c; 16]), u128::from_be_bytes([0x0d; 16])],
            tail_custody_digest: "98d1d2368b0d4c44f11dfc2e7c927bec75e8e2a9e3c97103de047dd45b1d9669".into(),
            observation_cut_digest: Some("0a".repeat(32)),
            checkpoint_generation: 3, checkpoint_payload_digest: "ab".repeat(32),
            published_snapshot_id: 5, applied_operation_id: "op-1".into(),
            applied_request_identity_digest: "c9682b0f4169c569c2af8d707c53fd3f53a7db7fcfe1688941131c22d6282c5c".into(),
            applied_source_owner_identity: "source-owner".into(),
            applied_source_execution_authority_digest: "source-execution".into(),
            applied_source_view_digest: "source-view".into(),
            applied_analysis_owner_identity: "analysis-owner".into(),
            applied_analysis_lineage_digest: "analysis-lineage".into(),
            applied_index_owner_identity: "index-owner".into(),
            applied_index_publication_identity: "index-publication".into(),
            applied_index_publication_digest: "index-digest".into(),
            applied_published_snapshot_id: 5,
        };
        assert_eq!(receipt.validate_request_identity_v1(), Ok(()));
        let mut foreign_tail = receipt.clone();
        foreign_tail.tail_event_ids[0] += 1;
        assert!(foreign_tail.validate_request_identity_v1().is_err());
        let mut foreign_cut = receipt.clone();
        foreign_cut.observation_cut_digest = Some("0b".repeat(32));
        assert!(foreign_cut.validate_request_identity_v1().is_err());
        let mut foreign_operation = receipt;
        foreign_operation.operation_id = "op-2".into();
        assert!(foreign_operation.validate_request_identity_v1().is_err());
    }

    #[test]
    fn completed_watch_wire_rejects_missing_extra_and_duplicate_fields_v1() {
        use ciborium::value::Value as V;
        for mode in 0..3 {
            let mut value = completed_watch_golden_v1();
            let V::Map(outer) = &mut value else { unreachable!() };
            let V::Map(fields) = &mut outer[0].1 else { unreachable!() };
            match mode {
                0 => { drop(fields.pop()); }
                1 => fields.push((V::Text("permission".into()), V::Bool(true))),
                _ => fields.push(fields[2].clone()),
            }
            let bytes = cbor_v1(&value);
            assert!(ciborium::de::from_reader::<NativeCutoverEvidenceSourceFactV1, _>(bytes.as_slice()).is_err());
        }
    }

    #[test]
    fn completed_watch_shape_refuses_foreign_source_duplicate_ids_and_capacity_v1() {
        let scope = watch_scope_v1();
        for mode in 0..8 {
            let mut fact = completed_watch_v1();
            let NativeCutoverEvidenceSourceFactV1::CompletedSourceRequest { native_operation: Some(operation), source, event_ids } = &mut fact
                else { unreachable!() };
            match mode {
                0 => operation.epoch += 1,
                1 => operation.producer = ProducerId([9; 16]),
                2 => source.transition_digest.0[0] = b'X',
                3 => source.target_snapshot_id = 0,
                4 => event_ids.clear(),
                5 => *event_ids = vec![1, 1],
                6 => *event_ids = vec![2, 1],
                _ => *event_ids = (0..=MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1).map(|id| id as u128).collect(),
            }
            assert!(fact.validate_source_publication_v1(&scope).is_err());
        }
    }

    #[test]
    fn v5_predecessor_pair_requires_two_lowercase_digests() {
        let previous = NativeCutoverDigest64V1([b'a'; 64]);
        let base = NativeCutoverDigest64V1([b'0'; 64]);
        let uppercase = NativeCutoverDigest64V1([b'A'; 64]);
        let non_hex = NativeCutoverDigest64V1([b'g'; 64]);
        assert_eq!(validate_v5_predecessor_pair_v1(None, None), Ok(()));
        assert_eq!(
            validate_v5_predecessor_pair_v1(Some(&previous), Some(&base)),
            Ok(())
        );
        for pair in [
            (Some(&previous), None),
            (None, Some(&base)),
            (Some(&uppercase), Some(&base)),
            (Some(&previous), Some(&non_hex)),
        ] {
            assert_eq!(
                validate_v5_predecessor_pair_v1(pair.0, pair.1),
                Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical),
            );
        }
    }

    #[test]
    fn v5_artifact_requires_exact_repomap_disposition_wire_v1() {
        use ciborium::value::Value as V;
        let V::Map(mut completed) = completed_watch_golden_v1() else { unreachable!() };
        let (_, V::Map(mut source_fields)) = completed.pop().unwrap() else { unreachable!() };
        let (_, operation_wire) = source_fields.remove(0);
        let NativeCutoverEvidenceSourceFactV1::CompletedSourceRequest {
            native_operation: Some(operation), ..
        } = completed_watch_v1() else { unreachable!() };
        let disposition = NativeCutoverEvidenceLegacyRepoMapV1::NotApplicable {
            reason_code: "no-repomap".to_string(),
            policy_digest: NativeCutoverDigest64V1([b'd'; 64]),
            product_scope_digest: NativeCutoverDigest64V1([b'e'; 64]),
        };
        let artifact = NativeCutoverEvidenceV5ArtifactV1 {
            operation,
            published_snapshot_id: 42,
            lexical_revision_digest: NativeCutoverDigest64V1([b'a'; 64]),
            prepared_artifact_digest: NativeCutoverDigest64V1([b'b'; 64]),
            aggregate_receipt_digest: NativeCutoverDigest64V1([b'c'; 64]),
            previous_transition_digest: None,
            base_source_digest: None,
            repomap: disposition.clone(),
        };
        let digest = |byte: u8| V::Array(vec![V::Integer(byte.into()); 64]);
        let expected = V::Map(vec![
            (V::Text("operation".into()), operation_wire),
            (V::Text("published_snapshot_id".into()), V::Integer(42.into())),
            (V::Text("lexical_revision_digest".into()), digest(b'a')),
            (V::Text("prepared_artifact_digest".into()), digest(b'b')),
            (V::Text("aggregate_receipt_digest".into()), digest(b'c')),
            (V::Text("previous_transition_digest".into()), V::Null),
            (V::Text("base_source_digest".into()), V::Null),
            (V::Text("repomap".into()), V::Map(vec![(
                V::Text("NotApplicable".into()), V::Map(vec![
                    (V::Text("reason_code".into()), V::Text("no-repomap".into())),
                    (V::Text("policy_digest".into()), digest(b'd')),
                    (V::Text("product_scope_digest".into()), digest(b'e')),
                ]),
            )])),
        ]);
        assert_eq!(cbor_v1(&artifact), cbor_v1(&expected));
        assert_eq!(ciborium::de::from_reader::<NativeCutoverEvidenceV5ArtifactV1, _>(
            cbor_v1(&expected).as_slice()).unwrap(), artifact);
        let V::Map(mut missing) = expected else { unreachable!() };
        drop(missing.pop());
        assert!(ciborium::de::from_reader::<NativeCutoverEvidenceV5ArtifactV1, _>(
            cbor_v1(&V::Map(missing)).as_slice()).is_err());
        let mut malformed = artifact;
        malformed.repomap = NativeCutoverEvidenceLegacyRepoMapV1::NotApplicable {
            reason_code: String::new(),
            policy_digest: NativeCutoverDigest64V1([b'd'; 64]),
            product_scope_digest: NativeCutoverDigest64V1([b'e'; 64]),
        };
        assert!(malformed.validate_predecessor_pair_v1().is_err());
    }

    #[test]
    fn syntax_refuses_duplicate_key_trailing_data_and_noncanonical_length() {
        assert_eq!(
            validate_native_cutover_evidence_cbor_v1(&[0xa2, 0x61, b'a', 0x01, 0x61, b'a', 0x02]),
            Err(NativeCutoverEvidenceSyntaxErrorV1::DuplicateField),
        );
        assert_eq!(
            validate_native_cutover_evidence_cbor_v1(&[0x80, 0x80]),
            Err(NativeCutoverEvidenceSyntaxErrorV1::TrailingData),
        );
        assert_eq!(
            validate_native_cutover_evidence_cbor_v1(&[0x98, 0x00]),
            Err(NativeCutoverEvidenceSyntaxErrorV1::NonCanonical),
        );
        assert_eq!(
            validate_native_cutover_evidence_cbor_v1(&[0xa1, 0x61, b'a', 0x01]),
            Ok(())
        );
    }

    #[test]
    fn duplicate_long_map_key_compares_content_after_length_prefix() {
        let mut duplicate = vec![0xa2, 0x78, 24];
        duplicate.extend_from_slice(&[b'a'; 24]);
        duplicate.push(0x01);
        duplicate.extend_from_slice(&[0x78, 24]);
        duplicate.extend_from_slice(&[b'a'; 24]);
        duplicate.push(0x02);
        assert_eq!(
            validate_native_cutover_evidence_cbor_v1(&duplicate),
            Err(NativeCutoverEvidenceSyntaxErrorV1::DuplicateField),
        );
        let last_key_byte = duplicate.len() - 2;
        duplicate[last_key_byte] = b'b';
        assert_eq!(validate_native_cutover_evidence_cbor_v1(&duplicate), Ok(()));
    }

    #[test]
    fn control_begin_terminal_cancel_are_closed_and_bounded() {
        let digest = Digest([7; 32]);
        let begin = NativeCutoverControlBeginV1 {
            format_version: 1,
            operation_id: "op-1".to_owned(),
            mode: NativeCutoverRecoveryModeV1::RollbackLegacy,
            current_selection_sha256: digest,
            candidate_selection_sha256: Digest([8; 32]),
            deadline_ns: 42,
            evidence_length: 12,
            evidence_sha256: Digest([9; 32]),
            nonce: Digest([10; 32]),
        };
        let mut begin_wire = Vec::new();
        ciborium::ser::into_writer(&begin, &mut begin_wire).expect("fixed begin encode");
        assert_eq!(
            decode_native_cutover_control_begin_v1(&begin_wire).expect("fixed begin decode"),
            begin
        );
        let terminal = NativeCutoverControlTerminalV1::PersistenceUncertain {
            operation_id: "op-1".to_owned(),
            nonce: begin.nonce,
            before_sha256: digest,
            after_sha256: None,
            code: "FSYNC_UNCERTAIN".to_owned(),
            message: "directory fsync did not return".to_owned(),
        };
        let mut terminal_wire = Vec::new();
        ciborium::ser::into_writer(&terminal, &mut terminal_wire).expect("fixed terminal encode");
        assert_eq!(
            decode_native_cutover_control_terminal_v1(&terminal_wire)
                .expect("fixed terminal decode"),
            terminal
        );
        let cancel = NativeCutoverControlCancelV1 {
            format_version: 1,
            operation_id: "op-1".to_owned(),
            nonce: begin.nonce,
        };
        let mut cancel_wire = Vec::new();
        ciborium::ser::into_writer(&cancel, &mut cancel_wire).expect("fixed cancel encode");
        assert_eq!(
            decode_native_cutover_control_cancel_v1(&cancel_wire).expect("fixed cancel decode"),
            cancel
        );

        let wrong_version = NativeCutoverControlCancelV1 {
            format_version: 2,
            ..cancel
        };
        let mut wrong_wire = Vec::new();
        ciborium::ser::into_writer(&wrong_version, &mut wrong_wire).expect("wrong version encode");
        assert!(decode_native_cutover_control_cancel_v1(&wrong_wire).is_err());
        let mut truncated = cancel_wire.clone();
        assert!(truncated.pop().is_some());
        assert!(decode_native_cutover_control_cancel_v1(&truncated).is_err());
        let mut trailing = cancel_wire;
        trailing.push(0xf6);
        assert!(decode_native_cutover_control_cancel_v1(&trailing).is_err());
        // Fixed CBOR maps with a missing required field and an unknown field.
        assert!(decode_native_cutover_control_cancel_v1(b"\xa1\x6eformat_version\x01").is_err());
        assert!(decode_native_cutover_control_cancel_v1(b"\xa1\x67unknown\x01").is_err());
        assert!(decode_native_cutover_control_cancel_v1(b"\xa2\x61x\x01\x61x\x02",).is_err());
    }

    #[test]
    fn digest64_requires_exact_tuple_length() {
        let digest = NativeCutoverDigest64V1([3; 64]);
        let mut wire = Vec::new();
        ciborium::ser::into_writer(&digest, &mut wire).expect("digest encode");
        let decoded: NativeCutoverDigest64V1 =
            ciborium::de::from_reader(wire.as_slice()).expect("digest decode");
        assert_eq!(decoded, digest);
        let mut short = vec![0x98, 63];
        short.extend_from_slice(&[3; 63]);
        assert!(ciborium::de::from_reader::<NativeCutoverDigest64V1, _>(short.as_slice()).is_err());
        let mut long = vec![0x98, 65];
        long.extend_from_slice(&[3; 65]);
        assert!(ciborium::de::from_reader::<NativeCutoverDigest64V1, _>(long.as_slice()).is_err());
    }

    #[test]
    fn host_u128_event_ids_use_canonical_positive_bignum() {
        // Independent RFC 8949 positive-bignum encoding of 2^127.
        let golden = [
            0xc2, 0x50, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let event_id = 1_u128 << 127;
        let mut scalar = Vec::new();
        ciborium::ser::into_writer(&event_id, &mut scalar).expect("canonical u128 encode");
        assert_eq!(scalar, golden);
        assert_eq!(validate_native_cutover_evidence_cbor_v1(&golden), Ok(()));

        // These are the same Host fact fields populated by the S exporter.
        let host = NativeCutoverEvidenceHostFactsV1 {
            workspace_authority: "workspace".to_owned(),
            capacity: 1,
            retained_delivery_count: 0,
            active_units: 0,
            prepared_authorities: Vec::new(),
            deliveries: Vec::new(),
            recovery_receipts: Vec::new(),
            pending_apply_intent: Some(NativeCutoverEvidencePendingApplyIntentV1 {
                verified_batch_identity: "batch".to_owned(),
                operation_id: "operation".to_owned(),
                workspace_tier: true,
                before_snapshot_id: 1,
                after_snapshot_id: 2,
                event_ids: vec![event_id, u128::MAX],
            }),
        };
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&host, &mut encoded).expect("Host facts encode");
        assert_eq!(validate_native_cutover_evidence_cbor_v1(&encoded), Ok(()));
        let decoded: NativeCutoverEvidenceHostFactsV1 =
            ciborium::de::from_reader(encoded.as_slice()).expect("Host facts decode");
        assert_eq!(decoded, host);
    }

    #[test]
    fn bignum_refuses_other_tags_nonminimal_payload_and_overflow() {
        for bytes in [
            &[0xc0, 0x49, 1, 0, 0, 0, 0, 0, 0, 0, 0][..],
            &[0xc3, 0x49, 1, 0, 0, 0, 0, 0, 0, 0, 0][..],
            &[0xc2, 0x48, 1, 0, 0, 0, 0, 0, 0, 0][..],
            &[0xc2, 0x49, 0, 1, 0, 0, 0, 0, 0, 0, 0][..],
            &[
                0xc2, 0x51, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ][..],
            &[0xc2, 0x01][..],
            &[0xc2, 0x49, 1][..],
            &[0xc2, 0x58, 9, 1, 0, 0, 0, 0, 0, 0, 0, 0][..],
        ] {
            assert!(validate_native_cutover_evidence_cbor_v1(bytes).is_err());
        }
        let minimum = [0xc2, 0x49, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(validate_native_cutover_evidence_cbor_v1(&minimum), Ok(()));
        let decoded: u128 = ciborium::de::from_reader(minimum.as_slice())
            .expect("smallest canonical positive bignum");
        assert_eq!(decoded, 1_u128 << 64);
    }

    #[test]
    fn before_document_uses_bounded_byte_string_and_reconciled_is_distinct() {
        let small = NativeCutoverByteStringV1(vec![1, 2, 3]);
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&small, &mut encoded).expect("byte string encode");
        assert_eq!(encoded, [0x43, 1, 2, 3]);
        let decoded: NativeCutoverByteStringV1 =
            ciborium::de::from_reader(encoded.as_slice()).expect("byte string decode");
        assert_eq!(decoded, small);

        let maximum = NativeCutoverByteStringV1(vec![7; 64 * 1024]);
        encoded.clear();
        ciborium::ser::into_writer(&maximum, &mut encoded).expect("64 KiB document");
        assert_eq!(validate_native_cutover_evidence_cbor_v1(&encoded), Ok(()));
        let decoded: NativeCutoverByteStringV1 =
            ciborium::de::from_reader(encoded.as_slice()).expect("64 KiB decode");
        assert_eq!(decoded, maximum);
        let mut excessive = vec![0x5a, 0, 1, 0, 1];
        excessive.resize(5 + 64 * 1024 + 1, 7);
        assert!(
            ciborium::de::from_reader::<NativeCutoverByteStringV1, _>(excessive.as_slice(),)
                .is_err()
        );
        assert!(
            ciborium::de::from_reader::<NativeCutoverByteStringV1, _>(&[0x83, 1, 2, 3][..],)
                .is_err()
        );

        let reconciled = NativeCutoverControlTerminalV1::Reconciled {
            operation_id: "stable-operation".to_owned(),
            nonce: Digest([1; 32]),
            before_sha256: Digest([2; 32]),
            after_sha256: Digest([3; 32]),
        };
        encoded.clear();
        ciborium::ser::into_writer(&reconciled, &mut encoded).expect("reconciled encode");
        let decoded =
            decode_native_cutover_control_terminal_v1(&encoded).expect("reconciled decode");
        assert_eq!(decoded, reconciled);
        assert!(!matches!(
            decoded,
            NativeCutoverControlTerminalV1::Applied { .. }
        ));
    }

    #[test]
    fn installed_history_terminal_preserves_the_exact_membership_binding() {
        use crate::index::{AuxiliaryGenerationKeyV1, CollectionId, SlotId, TenantId, WorktreeId};
        let workspace = WorkspaceKey {
            tenant: TenantId([4; 16]), collection: CollectionId([5; 16]),
            worktree: WorktreeId([6; 16]), slot: SlotId([7; 16]),
        };
        let mut history = NativeCutoverInstalledHistoryV1 {
            operation: OperationKey {
                workspace, epoch: 1, producer: ProducerId([8; 16]),
                stream: StreamId([9; 16]), stream_epoch: 1, sequence: 3,
            },
            view: ViewRef { workspace, epoch: 1, sequence: 6, digest: Digest([10; 32]) },
            instance: IndexInstanceId([11; 16]),
            member: ObjectRef(Digest([12; 32])),
            source_input: ObjectRef(Digest([13; 32])),
            cut: HistorySourceCutV1 {
                generation: AuxiliaryGenerationKeyV1 {
                    repo_id: crate::RepoId::new("r".repeat(crate::RepoId::MAX_UTF8_BYTES_V1)).expect("repo"),
                    revision_id: crate::RevisionId::new("v".repeat(crate::RevisionId::MAX_UTF8_BYTES_V1)).expect("revision"),
                    generation: crate::ManifestGeneration::new(1),
                },
                durable_epoch: crate::AuxEpochV1::new(2),
                snapshot_digest: Digest([14; 32]), replay_start: Digest([15; 32]),
            },
        };
        let mut bytes = Vec::new();
        let good = NativeCutoverControlTerminalV1::AppliedHistory {
            operation_id: "installed-history".to_owned(), nonce: Digest([1; 32]),
            before_sha256: Digest([2; 32]), after_sha256: Digest([3; 32]), history: history.clone(),
        };
        ciborium::ser::into_writer(&good, &mut bytes).expect("terminal encode");
        let decoded = decode_native_cutover_control_terminal_v1(&bytes).expect("terminal decode");
        assert_eq!(decoded, good);
        assert_eq!(decoded.installed_history_v1(), Some(&history));

        history.view.epoch = 2;
        let mismatched = NativeCutoverControlTerminalV1::ReconciledHistory {
            operation_id: "installed-history".to_owned(), nonce: Digest([1; 32]),
            before_sha256: Digest([2; 32]), after_sha256: Digest([3; 32]), history,
        };
        bytes.clear();
        ciborium::ser::into_writer(&mismatched, &mut bytes).expect("invalid terminal fixture encode");
        assert!(decode_native_cutover_control_terminal_v1(&bytes).is_err());
    }
}
