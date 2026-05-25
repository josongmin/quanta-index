#![no_main]
#![forbid(unsafe_code)]

//! Same shape as `ipc_request_decode.rs`, applied to the response side.

use libfuzzer_sys::fuzz_target;

use quanta_index_contract::{
    SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryResponse,
    SearchPlaneLexicalQueryResponse, SearchPlaneSemanticQueryResponse,
};

fuzz_target!(|data: &[u8]| {
    let _ = ciborium::de::from_reader::<SearchPlaneLexicalQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneSemanticQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneHybridQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneExplainQueryResponse, _>(data);
});
