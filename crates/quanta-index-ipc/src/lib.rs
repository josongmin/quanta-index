//! Search-plane IPC driving adapter — CBOR codec + `AF_UNIX` stream server/client.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

use quanta_index_core as _;

mod admission;
mod codec;
mod server;

pub use admission::{DispatchPermit, DispatchSlots, ServerAdmissionPolicy, SlotRefusal};
pub use codec::{
    IpcError, IpcIoOperation, MAX_FRAME_BODY_BYTES, decode_cbor_payload, decode_request,
    decode_response, encode_cbor_payload, encode_request, encode_response,
};
pub use quanta_index_core::{
    BudgetInterruptionV1, CancelHandleV1, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE,
    RequestBudgetV1,
};
pub use server::{
    ClientIoPolicy, DEFAULT_CLIENT_IO_TIMEOUT, IpcDispatcher, RequestEnvelope, ResponseEnvelope,
    ShutdownHandle, UdsServer, send_request,
};
