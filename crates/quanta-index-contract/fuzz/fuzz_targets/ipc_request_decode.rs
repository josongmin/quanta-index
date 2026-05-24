#![no_main]
#![forbid(unsafe_code)]

//! Differential fuzz target for IPC request decoders.
//!
//! Fail-closed deserialization is a contract invariant: the manual serde
//! impls in `quanta-index-contract::ipc::requests` MUST reject malformed
//! input as a typed Err rather than panicking, looping, or producing
//! placeholder values. This target throws arbitrary bytes at every request
//! decoder and lets libfuzzer flag any non-Err exit path.

use libfuzzer_sys::fuzz_target;

use quanta_index_contract::ipc::{
    SearchPlaneExplainQueryRequest, SearchPlaneHybridQueryRequest,
    SearchPlaneLexicalQueryRequest, SearchPlaneSemanticQueryRequest,
};

fuzz_target!(|data: &[u8]| {
    let _ = ciborium::de::from_reader::<SearchPlaneLexicalQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneSemanticQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneHybridQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneExplainQueryRequest, _>(data);
});
