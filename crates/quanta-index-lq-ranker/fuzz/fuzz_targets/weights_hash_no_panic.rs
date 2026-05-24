#![no_main]
#![forbid(unsafe_code)]

//! `weights_hash` no-panic fuzz target.
//!
//! `weights_hash` was rewritten from `-> [u8; 32]` (heuristic fallback) to
//! `-> Result<[u8; 32], RankerError>` per the repo `no silent fallback` rule.
//! This target locks the invariant: **no panic on any input**, error path
//! always typed.
//!
//! The whole pipeline (RankerWeightsV1::new -> weights_hash) must never
//! panic, never loop forever, never UB on any byte sequence.

use libfuzzer_sys::fuzz_target;

use quanta_index_lq_ranker::{RankerWeightsV1, weights_hash};

fuzz_target!(|data: &[u8]| {
    // Extract 5 f32s from the input. Skip if not enough bytes.
    if data.len() < 20 {
        return;
    }
    let bm25 = f32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let path_prior = f32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let symbol_boost = f32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    let recency = f32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    let boost_directive = f32::from_le_bytes([data[16], data[17], data[18], data[19]]);

    // Try to construct a weight set. If validation rejects, we exit cleanly.
    // The whole pipeline must never panic, never loop forever, never UB.
    if let Ok(w) = RankerWeightsV1::new(bm25, path_prior, symbol_boost, recency, boost_directive) {
        // weights_hash MUST always return Result; never panic.
        let _ = weights_hash(&w);
    }
});
