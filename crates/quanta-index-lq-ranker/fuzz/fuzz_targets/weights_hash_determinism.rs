#![no_main]
#![forbid(unsafe_code)]

//! `weights_hash` determinism fuzz target.
//!
//! Two calls to `weights_hash` for the same `RankerWeightsV1` must produce
//! byte-identical digests. Determinism is a hard invariant: the weight
//! identity / generation pin is downstream-observed via the digest, so any
//! drift would silently fork the cache key.

use libfuzzer_sys::fuzz_target;

use quanta_index_lq_ranker::{RankerWeightsV1, weights_hash};

fuzz_target!(|data: &[u8]| {
    if data.len() < 20 {
        return;
    }
    let bm25 = f32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let path_prior = f32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let symbol_boost = f32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    let recency = f32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    let boost_directive = f32::from_le_bytes([data[16], data[17], data[18], data[19]]);

    if let Ok(w) = RankerWeightsV1::new(bm25, path_prior, symbol_boost, recency, boost_directive) {
        let h1 = weights_hash(&w);
        let h2 = weights_hash(&w);
        assert_eq!(h1, h2, "weights_hash must be deterministic");
    }
});
