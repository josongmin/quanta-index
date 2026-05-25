#![no_main]
#![forbid(unsafe_code)]

//! Same shape as `ipc_request_decode.rs`, applied to the response side.

use libfuzzer_sys::fuzz_target;

use quanta_index_contract::{
    SearchPlaneExplainQueryResponse, HybridQueryResponse,
    TextQueryResponse, SemanticQueryResponse,
};

fuzz_target!(|data: &[u8]| {
    let _ = ciborium::de::from_reader::<TextQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SemanticQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<HybridQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneExplainQueryResponse, _>(data);
});
