#![no_main]
#![forbid(unsafe_code)]

//! `CompositeScorer::new` + `CompositeScorer::score` no-panic fuzz target.
//!
//! `CompositeScorer::new` now takes the Result path through `weights_hash`,
//! so it inherits the same `no silent fallback` invariant. This target
//! drives the full scorer pipeline (construction + score) against arbitrary
//! bytes and lets libfuzzer flag any panic / UB exit.

use libfuzzer_sys::fuzz_target;

use quanta_index_lq_ranker::{CandidateSignals, CompositeScorer, RankerWeightsV1};

fuzz_target!(|data: &[u8]| {
    if data.len() < 20 {
        return;
    }
    let bm25 = f32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let path_prior = f32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let symbol_boost = f32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    let recency = f32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    let boost_directive = f32::from_le_bytes([data[16], data[17], data[18], data[19]]);

    let Ok(w) = RankerWeightsV1::new(bm25, path_prior, symbol_boost, recency, boost_directive)
    else {
        return;
    };

    // CompositeScorer::new also takes the Result path now. Fuzz it directly.
    let Ok(scorer) = CompositeScorer::new(w) else {
        return;
    };

    // If remaining bytes allow extracting CandidateSignals (5 more f32s), call score + explain.
    if data.len() >= 40 {
        let sig_bm25 = f32::from_le_bytes([data[20], data[21], data[22], data[23]]);
        let sig_path_prior = f32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        let sig_symbol_boost = f32::from_le_bytes([data[28], data[29], data[30], data[31]]);
        let sig_recency = f32::from_le_bytes([data[32], data[33], data[34], data[35]]);
        let sig_boost_directive = f32::from_le_bytes([data[36], data[37], data[38], data[39]]);
        let sig = CandidateSignals {
            bm25: sig_bm25,
            path_prior: sig_path_prior,
            symbol_boost: sig_symbol_boost,
            recency: sig_recency,
            boost_directive: sig_boost_directive,
        };
        let _ = scorer.score(&sig);
        let _ = scorer.explain(&sig);
    }
});
