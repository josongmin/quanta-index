//! Native emission custody. Only canonical DTOs are serialized; scoped Text
//! stages share their immutable data through the complete synchronous write.

use quanta_index_contract::native_protocol::{
    NativeIndexResponse, NativeIndexResponseEnvelope, NativeIndexWireError,
};
use quanta_index_core::index::{IndexError, MemoryLease};
use quanta_index_ipc::{ResponseEnvelope, SlotRefusal};
use serde::{Serialize, Serializer};

use crate::native::{ScopedNativeLiteralPage, ScopedNativePathPage};
use crate::native_producer_source_lookup::ScopedNativeProducerSourcePage;
use crate::native_projection_owner_lookup::ScopedNativeProjectionOwnerLookup;
use crate::native_semantic_result::ScopedNativeSemanticResult;
use crate::native_source_owner_lookup::ScopedNativeSourceOwnerLookup;
use crate::native_text_result::ScopedNativeTextResult;

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

pub(crate) enum NativeReply {
    Plain(ScopedCanonicalReply),
    Error(NativeIndexWireError),
    Literal(ScopedNativeLiteralPage),
    Path(ScopedNativePathPage),
    Text(ScopedNativeTextResult),
    OverlayText(crate::native_overlay_query::ScopedOverlayTextResult),
    Semantic(ScopedNativeSemanticResult),
    OverlaySemantic(crate::native_semantic_result::ScopedOverlaySemanticResult),
    EffortDone(crate::native_effort_transport::ScopedNativeEffortDone),
    Graph(crate::native_graph_transport::ScopedNativeGraphResult),
    RepoMapRanked(crate::native_repomap_transport::ScopedNativeRepoMapRankResult),
    Overlay(crate::native_overlay_transport::ScopedOverlayReply),
    Stub(crate::native_stub_transport::ScopedStubReply),
    SourceOwners(ScopedNativeSourceOwnerLookup),
    ProjectionOwners(ScopedNativeProjectionOwnerLookup),
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
            | Self::EffortDone(_)
            | Self::SourceOwners(_)
            | Self::ProjectionOwners(_)
            | Self::ProducerSourceOwners(_)
            | Self::ProjectionSourceDiff(_)
            | Self::Graph(_)
            | Self::RepoMapRanked(_)
            | Self::Overlay(_)
            | Self::Stub(_)
            | Self::Literal(_)
            | Self::Path(_)
            | Self::StagedSources(_) => false,
        }
    }

    fn terminal(&self) -> bool {
        match self {
            Self::Plain(payload) => payload.result.as_ref().as_ref().is_some_and(
                <NativeIndexResponseEnvelope as ResponseEnvelope<NativeIndexResponse>>::stream_terminal,
            ),
            Self::EffortDone(_) | Self::Error(_) => true,
            Self::Text(_)
            | Self::Semantic(_)
            | Self::OverlayText(_)
            | Self::OverlaySemantic(_)
            | Self::SourceOwners(_)
            | Self::ProjectionOwners(_)
            | Self::ProducerSourceOwners(_)
            | Self::ProjectionSourceDiff(_)
            | Self::Graph(_)
            | Self::RepoMapRanked(_)
            | Self::Overlay(_)
            | Self::Stub(_)
            | Self::StagedSources(_)
            | Self::Literal(_)
            | Self::Path(_) => false,
        }
    }
}

impl Serialize for NativeReply {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Plain(payload) => payload.serialize(serializer),
            Self::Error(error) => NativeIndexResponse::serialize_error_ref(error, serializer),
            Self::Literal(result) => {
                NativeIndexResponse::serialize_literal_page_ref(result, serializer)
            }
            Self::Path(result) => NativeIndexResponse::serialize_path_page_ref(result, serializer),
            Self::OverlayText(result) => {
                NativeIndexResponse::serialize_overlay_text_ref(result, serializer)
            }
            Self::Text(result) => {
                NativeIndexResponse::serialize_text_ranked_ref(result, serializer)
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
            Self::ProjectionSourceDiff(result) => {
                NativeIndexResponse::serialize_projection_source_diff_ref(result, serializer)
            }
            Self::ProducerSourceOwners(result) => {
                NativeIndexResponse::serialize_producer_source_owners_ref(result, serializer)
            }
            Self::Graph(result) => NativeIndexResponse::serialize_graph_ref(result, serializer),
            Self::RepoMapRanked(result) => result.serialize(serializer),
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
    use quanta_index_contract::native_protocol::{
        NativeIndexErrorCode, NativeInteractionPublisherId,
    };
    use quanta_index_core::index::MemoryLeaseOwner;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

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
}
