//! Native emission custody. Only canonical DTOs are serialized; scoped Text
//! stages share their immutable data through the complete synchronous write.

use quanta_index_contract::index::{OperationStatus, SubmitOutcome};
use quanta_index_contract::native_protocol::{
    NativeEffortDone, NativeIndexResponse, NativeIndexResponseEnvelope, NativeIndexWireError,
    NativeReadSessionId,
};
use quanta_index_core::index::{IndexError, MemoryLease};
use quanta_index_ipc::{ResponseEnvelope, SlotRefusal};
use serde::{Serialize, Serializer};

use crate::native::{ScopedNativeLiteralPage, ScopedNativePathPage};
use crate::native_producer_source_lookup::ScopedNativeProducerSourcePage;
use crate::native_projection_owner_lookup::ScopedNativeProjectionOwnerLookup;
use crate::native_semantic_result::ScopedNativeSemanticResult;
use crate::native_source_owner_lookup::ScopedNativeSourceOwnerLookup;
use crate::native_text_result::{
    ScopedNativeTextPathPage, ScopedNativeTextPathResult, ScopedNativeTextPreviewResult,
    ScopedNativeTextResult,
};

/// Canonical success DTO storage is funded before its Box is born. Refusals
/// remain inline so resource denial never requires another response grant.
pub(crate) struct ScopedCanonicalReply {
    result: Box<Option<NativeIndexResponse>>,
    _memory: Option<MemoryLease>,
}

impl Serialize for ScopedCanonicalReply {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(payload) = self.result.as_ref() else {
            return Err(serde::ser::Error::custom(
                "canonical reply slot was not filled",
            ));
        };
        payload.serialize(serializer)
    }
}

pub(crate) struct ScopedNativeEffortDone {
    result: NativeEffortDone,
    _memory: Option<MemoryLease>,
}

impl ScopedNativeEffortDone {
    pub(crate) fn new(result: NativeEffortDone, memory: Option<MemoryLease>) -> Self {
        Self {
            result,
            _memory: memory,
        }
    }
}

impl std::ops::Deref for ScopedNativeEffortDone {
    type Target = NativeEffortDone;
    fn deref(&self) -> &Self::Target {
        &self.result
    }
}

pub(crate) enum NativeReply {
    Plain(ScopedCanonicalReply),
    Error(NativeIndexWireError),
    ReadSessionProfile {
        session: NativeReadSessionId,
        profile: quanta_index_kernel::ReadSessionProfile,
    },
    ReadSessionHistoryCut {
        value: quanta_index_contract::native_protocol::NativeReadSessionHistoryCutV1,
        _memory: Option<MemoryLease>,
    },
    Literal(ScopedNativeLiteralPage),
    Path(ScopedNativePathPage),
    RepositoryPath(quanta_index_repomap::native::RepositoryFilePathPage),
    Text(ScopedNativeTextResult),
    TextPreview(ScopedNativeTextPreviewResult),
    TextPaths(ScopedNativeTextPathResult),
    TextPathPage(ScopedNativeTextPathPage),
    TextPage(crate::native_read_session_transport::ScopedNativeTextPage),
    HybridFile(crate::native_semantic_result::ScopedNativeHybridFileResult),
    HistoryPage(crate::native_read_session_transport::ScopedNativeHistoryPage),
    DslPageV1(crate::native_read_session_transport::ScopedNativeDslPage),
    OverlayText(crate::native_overlay_query::ScopedOverlayTextResult),
    Semantic(ScopedNativeSemanticResult),
    OverlaySemantic(crate::native_semantic_result::ScopedOverlaySemanticResult),
    EffortDone(ScopedNativeEffortDone),
    Graph(crate::native_graph_transport::ScopedNativeGraphResult),
    Overlay(crate::native_overlay_transport::ScopedOverlayReply),
    Stub(crate::native_stub_transport::ScopedStubReply),
    SourceOwners(ScopedNativeSourceOwnerLookup),
    ProjectionOwners(ScopedNativeProjectionOwnerLookup),
    ProducerProjectionOwnersV1(
        crate::native_projection_owner_lookup::ScopedProducerProjectionOwnersV1,
    ),
    ProducerSourceOwners(ScopedNativeProducerSourcePage),
    ProjectionSourceDiff(crate::native_projection_source_diff::ScopedNativeProjectionSourceDiff),
    StagedSources(quanta_index_core::index::StagedObjectReferences),
}

pub(crate) struct CanonicalReplySlot {
    storage: Box<Option<NativeIndexResponse>>,
    memory: Option<MemoryLease>,
}

impl CanonicalReplySlot {
    pub(crate) fn finish(mut self, payload: NativeIndexResponse) -> NativeReply {
        if let NativeIndexResponse::Error(error) = payload {
            return NativeReply::Error(error);
        }
        *self.storage = Some(payload);
        NativeReply::Plain(ScopedCanonicalReply {
            result: self.storage,
            _memory: self.memory,
        })
    }
}

impl NativeReply {
    /// Reserve fixed DTO storage before executing an operation with side effects.
    pub(crate) fn reserve_canonical(
        reserve: impl FnOnce(u64) -> Result<Option<MemoryLease>, IndexError>,
    ) -> Result<CanonicalReplySlot, IndexError> {
        let bytes = u64::try_from(std::mem::size_of::<Option<NativeIndexResponse>>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let memory = reserve(bytes)?;
        Ok(CanonicalReplySlot {
            storage: Box::new(None),
            memory,
        })
    }

    fn stage(&self) -> bool {
        match self {
            Self::Plain(payload) => payload.result.as_ref().as_ref().is_some_and(
                <NativeIndexResponseEnvelope as ResponseEnvelope<NativeIndexResponse>>::stream_stage,
            ),
            Self::Text(_) | Self::Semantic(_) | Self::OverlayText(_) | Self::OverlaySemantic(_) => {
                true
            }
            Self::Error(_)
            | Self::TextPreview(_)
            | Self::TextPaths(_)
            | Self::TextPathPage(_)
            | Self::ReadSessionProfile { .. }
            | Self::ReadSessionHistoryCut { .. }
            | Self::EffortDone(_)
            | Self::SourceOwners(_)
            | Self::ProjectionOwners(_)
            | Self::ProducerProjectionOwnersV1(_)
            | Self::ProducerSourceOwners(_)
            | Self::ProjectionSourceDiff(_)
            | Self::Graph(_)
            | Self::Overlay(_)
            | Self::Stub(_)
            | Self::Literal(_)
            | Self::TextPage(_)
            | Self::HybridFile(_)
            | Self::DslPageV1(_)
            | Self::HistoryPage(_)
            | Self::Path(_)
            | Self::RepositoryPath(_)
            | Self::StagedSources(_) => false,
        }
    }

    fn terminal(&self) -> bool {
        match self {
            Self::Plain(payload) => payload.result.as_ref().as_ref().is_some_and(
                <NativeIndexResponseEnvelope as ResponseEnvelope<NativeIndexResponse>>::stream_terminal,
            ),
            Self::EffortDone(_) | Self::Error(_) => true,
            Self::ReadSessionProfile { .. } | Self::ReadSessionHistoryCut { .. } => false,
            Self::Text(_)
            | Self::TextPreview(_)
            | Self::TextPaths(_)
            | Self::TextPathPage(_)
            | Self::Semantic(_)
            | Self::OverlayText(_)
            | Self::OverlaySemantic(_)
            | Self::SourceOwners(_)
            | Self::ProjectionOwners(_)
            | Self::ProducerProjectionOwnersV1(_)
            | Self::ProducerSourceOwners(_)
            | Self::ProjectionSourceDiff(_)
            | Self::Graph(_)
            | Self::Overlay(_)
            | Self::Stub(_)
            | Self::StagedSources(_)
            | Self::Literal(_)
            | Self::TextPage(_)
            | Self::HybridFile(_)
            | Self::DslPageV1(_)
            | Self::HistoryPage(_)
            | Self::Path(_)
            | Self::RepositoryPath(_) => false,
        }
    }
}

impl Serialize for NativeReply {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Plain(payload) => payload.serialize(serializer),
            Self::Error(error) => NativeIndexResponse::serialize_error_ref(error, serializer),
            Self::ReadSessionProfile { session, profile } => {
                NativeIndexResponse::serialize_read_session_profile_ref(
                    *session,
                    profile.view(),
                    profile.profile(),
                    serializer,
                )
            }
            Self::ReadSessionHistoryCut { value, .. } => {
                serializer.serialize_newtype_variant("NativeIndexResponse", 40, "HistoryCut", value)
            }
            Self::TextPage(result) => serializer.serialize_newtype_variant(
                "NativeIndexResponse",
                35,
                "TextRankedPage",
                &**result,
            ),
            Self::HistoryPage(result) => result.serialize(serializer),
            Self::DslPageV1(result) => result.serialize(serializer),
            Self::HybridFile(result) => serializer.serialize_newtype_variant(
                "NativeIndexResponse",
                34,
                "HybridFileRanked",
                &**result,
            ),
            Self::Literal(result) => {
                NativeIndexResponse::serialize_literal_page_ref(result, serializer)
            }
            Self::Path(result) => NativeIndexResponse::serialize_path_page_ref(result, serializer),
            Self::RepositoryPath(result) => {
                NativeIndexResponse::serialize_repository_path_page_ref(&**result, serializer)
            }
            Self::OverlayText(result) => {
                NativeIndexResponse::serialize_overlay_text_ref(result, serializer)
            }
            Self::Text(result) => {
                NativeIndexResponse::serialize_text_ranked_ref(result, serializer)
            }
            Self::TextPreview(result) => {
                NativeIndexResponse::serialize_text_preview_ranked_ref(result, serializer)
            }
            Self::TextPaths(result) => {
                NativeIndexResponse::serialize_text_path_ranked_ref(result, serializer)
            }
            Self::TextPathPage(result) => {
                NativeIndexResponse::serialize_text_path_page_ref(result, serializer)
            }
            Self::OverlaySemantic(result) => {
                NativeIndexResponse::serialize_overlay_semantic_ref(result, serializer)
            }
            Self::EffortDone(result) => {
                NativeIndexResponse::serialize_effort_done_ref(result, serializer)
            }
            Self::Semantic(result) => {
                NativeIndexResponse::serialize_semantic_ranked_ref(result, serializer)
            }
            Self::SourceOwners(result) => {
                NativeIndexResponse::serialize_source_owners_ref(result, serializer)
            }
            Self::ProjectionOwners(result) => {
                NativeIndexResponse::serialize_projection_owners_ref(result, serializer)
            }
            Self::ProducerProjectionOwnersV1(result) => serializer.serialize_newtype_variant(
                "NativeIndexResponse",
                72,
                "ProducerProjectionOwnersV1",
                &**result,
            ),
            Self::ProjectionSourceDiff(result) => {
                NativeIndexResponse::serialize_projection_source_diff_ref(result, serializer)
            }
            Self::ProducerSourceOwners(result) => {
                NativeIndexResponse::serialize_producer_source_owners_ref(result, serializer)
            }
            Self::Graph(result) => NativeIndexResponse::serialize_graph_ref(result, serializer),
            Self::Overlay(result) => NativeIndexResponse::serialize_overlay_ref(result, serializer),
            Self::Stub(result) => NativeIndexResponse::serialize_stub_ref(result, serializer),
            Self::StagedSources(result) => {
                NativeIndexResponse::serialize_staged_sources_ref(result, serializer)
            }
        }
    }
}

pub(crate) struct NativeReplyEnvelope {
    request_id: u64,
    payload: NativeReply,
}

impl NativeReplyEnvelope {
    fn from_canonical_error(envelope: NativeIndexResponseEnvelope) -> Option<Self> {
        let NativeIndexResponse::Error(error) = envelope.payload else {
            return None;
        };
        Some(Self {
            request_id: envelope.request_id,
            payload: NativeReply::Error(error),
        })
    }
}

impl Serialize for NativeReplyEnvelope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        NativeIndexResponseEnvelope::serialize_payload_ref(
            self.request_id,
            &self.payload,
            serializer,
        )
    }
}

impl ResponseEnvelope<NativeReply> for NativeReplyEnvelope {
    fn from_parts(request_id: u64, payload: NativeReply) -> Self {
        Self {
            request_id,
            payload,
        }
    }

    fn terminal_reply_phase(&self) -> bool {
        match &self.payload {
            NativeReply::Error(_) => true,
            NativeReply::Plain(reply) => matches!(
                reply.result.as_ref().as_ref(),
                Some(NativeIndexResponse::Error(_))
                    | Some(NativeIndexResponse::Submitted(
                        quanta_index_contract::index::OperationReply {
                            payload: SubmitOutcome::Unknown(_)
                                | SubmitOutcome::Terminal(
                                    OperationStatus::Published(_) | OperationStatus::Refused(_)
                                ),
                            ..
                        }
                    ))
            ),
            NativeReply::Literal(_)
            | NativeReply::ReadSessionProfile { .. }
            | NativeReply::ReadSessionHistoryCut { .. }
            | NativeReply::Path(_)
            | NativeReply::RepositoryPath(_)
            | NativeReply::Text(_)
            | NativeReply::TextPreview(_)
            | NativeReply::TextPaths(_)
            | NativeReply::TextPathPage(_)
            | NativeReply::TextPage(_)
            | NativeReply::DslPageV1(_)
            | NativeReply::HistoryPage(_)
            | NativeReply::HybridFile(_)
            | NativeReply::OverlayText(_)
            | NativeReply::Semantic(_)
            | NativeReply::OverlaySemantic(_)
            | NativeReply::EffortDone(_)
            | NativeReply::Graph(_)
            | NativeReply::Overlay(_)
            | NativeReply::Stub(_)
            | NativeReply::SourceOwners(_)
            | NativeReply::ProjectionOwners(_)
            | NativeReply::ProducerProjectionOwnersV1(_)
            | NativeReply::ProducerSourceOwners(_)
            | NativeReply::ProjectionSourceDiff(_)
            | NativeReply::StagedSources(_) => false,
        }
    }

    fn stream_unsupported(request_id: u64) -> Option<Self> {
        <NativeIndexResponseEnvelope as ResponseEnvelope<NativeIndexResponse>>::stream_unsupported(
            request_id,
        )
        .and_then(Self::from_canonical_error)
    }

    fn stream_stage(payload: &NativeReply) -> bool {
        payload.stage()
    }

    fn stream_terminal(payload: &NativeReply) -> bool {
        payload.terminal()
    }

    fn result_too_large(request_id: u64, encoded_bytes: u64, limit_bytes: u64) -> Option<Self> {
        <NativeIndexResponseEnvelope as ResponseEnvelope<NativeIndexResponse>>::result_too_large(
            request_id,
            encoded_bytes,
            limit_bytes,
        )
        .and_then(Self::from_canonical_error)
    }

    fn overloaded(request_id: u64, refusal: &SlotRefusal) -> Option<Self> {
        <NativeIndexResponseEnvelope as ResponseEnvelope<NativeIndexResponse>>::overloaded(
            request_id, refusal,
        )
        .and_then(Self::from_canonical_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::{
        AttemptId, AttemptRef, CollectionId, Digest, IndexInstanceId, ObjectRef, OperationKey,
        OperationReply, OperationStatus, ProducerId, Refusal, SlotId, StreamId, SubmitOutcome,
        TenantId, ViewRef, WorkspaceKey, WorktreeId,
    };
    use quanta_index_contract::native_protocol::{
        NativeIndexErrorCode, NativeIndexRequest, NativeIndexRequestEnvelope,
        NativeInteractionPublisherId, NativeTextCoverage, NativeTextResult,
    };
    use quanta_index_core::index::MemoryLeaseOwner;
    use quanta_index_ipc::{
        ClientIoPolicy, DispatchContextV1, IpcDispatcher, IpcPlane, RequestBudgetV1,
        ServerAdmissionPolicy, UdsServer, send_request,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::Duration;

    struct LiveMemory(AtomicU64);
    impl MemoryLeaseOwner for LiveMemory {
        fn release(&self, bytes: u64) {
            let _prior = self.0.fetch_sub(bytes, Ordering::SeqCst);
        }
    }

    #[test]
    fn canonical_response_reserves_before_box_and_retains_through_serialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let expected =
            NativeIndexResponse::InteractionAccepted(NativeInteractionPublisherId([7; 32]));
        let bytes = u64::try_from(std::mem::size_of::<Option<NativeIndexResponse>>())?;
        let live = Arc::new(LiveMemory(AtomicU64::new(0)));
        let slot = NativeReply::reserve_canonical(|requested| {
            if requested != bytes {
                return Err(IndexError::BackendViolation(
                    "canonical response sizing changed".into(),
                ));
            }
            live.0.store(requested, Ordering::SeqCst);
            Ok(Some(MemoryLease::new(requested, live.clone())))
        })?;
        if live.0.load(Ordering::SeqCst) != bytes {
            return Err("canonical response did not retain its reserved bytes".into());
        }
        let reply = slot.finish(expected.clone());
        let mut actual_wire = Vec::new();
        let mut expected_wire = Vec::new();
        ciborium::ser::into_writer(&reply, &mut actual_wire)?;
        ciborium::ser::into_writer(&expected, &mut expected_wire)?;
        if actual_wire != expected_wire {
            return Err("canonical response wire differs from expected response".into());
        }
        if live.0.load(Ordering::SeqCst) != bytes {
            return Err("serialization lost the canonical response reservation".into());
        }
        drop(reply);
        if live.0.load(Ordering::SeqCst) != 0 {
            return Err("canonical response reservation survived reply drop".into());
        }
        if !matches!(
            NativeReply::reserve_canonical(|_bytes| Err(IndexError::ResourceLimited)),
            Err(IndexError::ResourceLimited)
        ) {
            return Err("canonical response reservation refusal changed".into());
        }
        Ok(())
    }

    #[test]
    fn refusal_wire_needs_no_canonical_storage_grant() -> Result<(), Box<dyn std::error::Error>> {
        let expected = NativeIndexResponse::Error(NativeIndexWireError {
            code: NativeIndexErrorCode::ResourceLimited,
            message: "capacity exhausted".into(),
        });
        let NativeIndexResponse::Error(error) = &expected else {
            return Err("refusal fixture lost its error DTO".into());
        };
        let reply = NativeReply::Error(error.clone());
        let mut actual_wire = Vec::new();
        let mut expected_wire = Vec::new();
        ciborium::ser::into_writer(&reply, &mut actual_wire)?;
        ciborium::ser::into_writer(&expected, &mut expected_wire)?;
        if actual_wire != expected_wire {
            return Err("refusal wire differs from expected error response".into());
        }
        if reply.stage() || !reply.terminal() {
            return Err("refusal reply stage or terminal state changed".into());
        }
        Ok(())
    }

    fn plain_reply(payload: NativeIndexResponse) -> NativeReplyEnvelope {
        NativeReplyEnvelope::from_parts(
            7,
            NativeReply::Plain(ScopedCanonicalReply {
                result: Box::new(Some(payload)),
                _memory: None,
            }),
        )
    }

    #[test]
    fn terminal_reply_phase_only_covers_closed_native_results() {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let operation = OperationKey {
            workspace,
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence: 1,
        };
        let attempt = AttemptRef {
            operation,
            attempt: AttemptId([7; 16]),
            fence: 1,
        };
        let view = ViewRef {
            workspace,
            epoch: 1,
            sequence: 1,
            digest: Digest([8; 32]),
        };
        let submitted = |payload| {
            plain_reply(NativeIndexResponse::Submitted(OperationReply {
                operation,
                payload,
            }))
        };

        assert!(
            NativeReplyEnvelope::from_parts(
                7,
                NativeReply::Error(NativeIndexWireError {
                    code: NativeIndexErrorCode::Cancelled,
                    message: "dispatch expired".into(),
                }),
            )
            .terminal_reply_phase()
        );
        assert!(
            plain_reply(NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                message: "dispatch expired".into(),
            }))
            .terminal_reply_phase()
        );
        assert!(submitted(SubmitOutcome::Unknown(operation)).terminal_reply_phase());
        assert!(
            submitted(SubmitOutcome::Terminal(OperationStatus::Published(view)))
                .terminal_reply_phase()
        );
        assert!(
            submitted(SubmitOutcome::Terminal(OperationStatus::Refused(
                Refusal::ResourceLimited,
            )))
            .terminal_reply_phase()
        );

        assert!(!submitted(SubmitOutcome::Accepted(attempt)).terminal_reply_phase());
        assert!(
            !submitted(SubmitOutcome::Terminal(OperationStatus::Pending(attempt)))
                .terminal_reply_phase()
        );
        assert!(
            !submitted(SubmitOutcome::Terminal(OperationStatus::Unknown)).terminal_reply_phase()
        );
        assert!(
            !plain_reply(NativeIndexResponse::TextRanked(NativeTextResult {
                view,
                instance: IndexInstanceId([9; 16]),
                coverage: NativeTextCoverage::CoreRequired,
                artifact: ObjectRef(Digest([10; 32])),
                matched: 0,
                exhaustive: true,
                hits: Vec::new(),
            }))
            .terminal_reply_phase()
        );
    }

    struct ExpiredNativeDispatcher;

    impl IpcDispatcher<NativeIndexRequest, NativeReply> for ExpiredNativeDispatcher {
        fn dispatch(
            &self,
            _context: &DispatchContextV1,
            _request: NativeIndexRequest,
            budget: &RequestBudgetV1,
        ) -> NativeReply {
            std::thread::sleep(Duration::from_millis(80));
            let expired = budget.checkpoint("native-terminal-reply-test").is_err();
            NativeReply::Error(NativeIndexWireError {
                code: if expired {
                    NativeIndexErrorCode::Cancelled
                } else {
                    NativeIndexErrorCode::BackendViolation
                },
                message: "native dispatch deadline reached".into(),
            })
        }
    }

    #[test]
    fn expired_native_dispatch_delivers_typed_error_over_uds()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = quanta_index_searchd_harness::private_tempdir()?;
        let socket = dir.path().join("native-reply.sock");
        let policy = ServerAdmissionPolicy::new(
            2,
            1,
            1,
            Duration::ZERO,
            Duration::from_millis(20),
            Duration::from_secs(2),
        )?;
        let server = Arc::new(UdsServer::bind_with_policy(&socket, policy)?);
        let running = Arc::clone(&server);
        let join = std::thread::spawn(move || {
            running.run::<
                NativeIndexRequestEnvelope,
                NativeIndexRequest,
                NativeReplyEnvelope,
                NativeReply,
                ExpiredNativeDispatcher,
            >(&Arc::new(ExpiredNativeDispatcher), IpcPlane::Control, Duration::from_millis(1))
        });
        let request = NativeIndexRequestEnvelope {
            request_id: 7,
            payload: NativeIndexRequest::InteractionBegin { window_millis: 1 },
        };
        let response = send_request::<NativeIndexRequestEnvelope, NativeIndexResponseEnvelope>(
            &socket,
            &request,
            ClientIoPolicy::try_new(Duration::from_secs(5))?,
        );
        server.shutdown_handle().trigger();
        join.join()
            .map_err(|panic| format!("native UDS server panicked: {panic:?}"))??;
        let response = response?;
        assert_eq!(response.request_id, request.request_id);
        assert!(matches!(
            response.payload,
            NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            })
        ));
        Ok(())
    }
}
