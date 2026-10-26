//! Search-plane IPC driving adapter — CBOR codec + `AF_UNIX` stream server/client.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

#[cfg(not(unix))]
compile_error!(
    "quanta-index-ipc requires a native transport with an explicit named-pipe ACL and kernel peer identity check; the Unix socket implementation cannot serve Windows"
);

mod admission;
mod admitted_decode;
mod batch_digest;
mod cbor_preflight;
mod codec;
mod counters;
mod error;
mod native_protocol;
mod payload_admission;
mod peer_credentials;
mod plane;
mod server;
mod socket_access;
mod source_upload;
pub use source_upload::{
    SOURCE_PUBLICATION_INLINE_BYTES, SOURCE_PUBLICATION_UPLOAD_STAGING_SLOTS,
    SourcePublicationUploadError, SourcePublicationUploadParent, SourcePublicationUploadStore,
    for_each_source_publication_upload_part, source_publication_upload_identity,
};

pub use admission::{
    DispatchPermit, DispatchSlots, IngressBudget, ServerAdmissionPolicy, SlotRefusal,
};
pub use admitted_decode::{
    AdmittedCborDecodeErrorV1, AdmittedDecodedCborV1, decode_cbor_seed_admitted_v1,
};
pub use batch_digest::{
    BatchDigestVerdictV1, CanonicalBatchDigestProgressError, canonical_batch_digest_v1,
    canonical_batch_digest_with_progress_v1, matches_batch_digest_token_v1, stamp_batch_digest_v1,
    verify_batch_digest_v1,
};
pub use codec::{
    AdmittedClientRequestFrame, AdmittedClientResponseBody, CborPayloadLengthProgressErrorV1,
    CborPayloadMatchProgressError, MAX_MULTIFRAME_REQUEST_BODY_BYTES, RequestSizeDecisionV1,
    ResponseFrame, cbor_payload_len, cbor_payload_len_with_progress_v1, cbor_payload_matches,
    cbor_payload_matches_with_progress, cbor_payload_matches_with_work_v1, decode_cbor_payload,
    decode_request, decode_response, encode_cbor_payload, encode_request,
    encode_request_single_frame_admitted, encode_response, encode_response_admitted,
    read_response_body_admitted, read_response_body_admitted_with_work_v1,
    request_size_decision_v1,
};
pub use counters::{
    IpcServerCounters, IpcServerCountersSnapshot, RequestEventSinkV1, RequestEventStageV1,
    RequestEventV1, RequestEventWindowV1, SequencedRequestEventV1,
};
pub use error::{IpcError, IpcIoOperation, MAX_FRAME_BODY_BYTES};
pub use payload_admission::{
    CborPayloadAdmissionErrorV1, encode_cbor_payload_admitted_v1, encode_cbor_payload_into_slots_v1,
};
pub use peer_credentials::{KernelPeerCredentials, PeerCredentialsSource};
pub use plane::IpcPlane;
pub use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
};
pub use quanta_index_core::{
    BudgetInterruptionV1, CancelHandleV1, IngestBatchBodyV1, REQUEST_CANCELLED_CODE,
    REQUEST_DEADLINE_EXCEEDED_CODE, RequestBudgetV1, RequestExecutionObserverV1,
};
pub use server::{
    BoundSocketPathProbe, ClientIoPolicy, ClientIpcTimingV1, ClientResponseStream,
    DEFAULT_CLIENT_IO_TIMEOUT, DispatchContextV1, IpcDispatcher, RequestEnvelope, ResponseEnvelope,
    ShutdownHandle, StreamDispatch, UdsServer, send_request,
    send_request_body_negotiated_with_budget_admitted,
    send_request_body_negotiated_with_budget_admitted_with_work_v1,
    send_request_body_with_budget_admitted, send_request_body_with_budget_admitted_with_work_v1,
    send_request_negotiated, send_request_observed, send_request_observed_negotiated,
    send_request_with_budget, send_stream_request,
};
pub use socket_access::{
    GROUP_DIRECTORY_MODE, GROUP_SOCKET_MODE, PRIVATE_DIRECTORY_MODE, PRIVATE_SOCKET_MODE,
    PeerCredentials, PeerRefusal, SharedSocketAccess, SocketAccessPolicy, WORLD_DIRECTORY_MODE,
    WORLD_SOCKET_MODE, admit_peer,
};
