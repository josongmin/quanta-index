//! Search-plane IPC driving adapter — CBOR codec + `AF_UNIX` stream server/client.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

use quanta_index_core as _;

mod codec;
mod server;

pub use codec::{
    IpcError, MAX_FRAME_BODY_BYTES, decode_request, decode_response, encode_request,
    encode_response,
};
pub use server::{QueryDispatcher, ShutdownHandle, UdsServer, send_request};
