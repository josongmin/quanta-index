//! Property test — replay-safe idempotency of `upsert_doc`.
//!
//! Per the producer/search-plane delta-handling contract: a subscriber
//! that replays an unacked event window MUST produce the same final index
//! state as a clean run of the same events. Proptest 256 cases.

use std::collections::BTreeMap;

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_trigram::{DocId, TrigramIndexBuilder};

fn small_doc_bytes() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..=32)
}

fn op_strategy() -> impl Strategy<Value = (u64, Vec<u8>)> {
    (0u64..=8u64, small_doc_bytes())
}

fn ops_strategy() -> impl Strategy<Value = Vec<(u64, Vec<u8>)>> {
    vec(op_strategy(), 1..=24)
}

fn build_via_upserts(generation: u64, ops: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let Ok(mut b) = TrigramIndexBuilder::new(generation) else {
        std::process::abort();
    };
    for (id, bytes) in ops {
        if let Err(_e) = b.upsert_doc(DocId(*id), bytes) {
            std::process::abort();
        }
    }
    let idx = b.finish();
    let mut buf: Vec<u8> = Vec::new();
    if let Err(_e) = idx.serialize_cbor(&mut buf) {
        std::process::abort();
    }
    buf
}

fn final_state(ops: &[(u64, Vec<u8>)]) -> BTreeMap<u64, Vec<u8>> {
    // Per-doc-id, last write wins.
    let mut state: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    for (id, bytes) in ops {
        let _replaced: Option<Vec<u8>> = state.insert(*id, bytes.clone());
    }
    state
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Per-doc idempotency: re-applying the FINAL state of every doc_id
    /// (last-write-wins from `ops`) after running `ops` itself produces
    /// the same CBOR encoding as just running `ops`. This is the
    /// crash-replay shape: the subscriber drains the channel, crashes
    /// before acking, replays the same events, and ends up in the same
    /// state.
    #[test]
    fn replay_of_final_state_is_byte_identical(
        generation in 1u64..=10_000u64,
        ops in ops_strategy(),
    ) {
        // Build A: apply ops as given.
        let buf_once = build_via_upserts(generation, &ops);
        // Build B: apply ops, then replay the canonical final state on
        // top. Because upsert_doc is idempotent, this must match buf_once.
        let mut combined: Vec<(u64, Vec<u8>)> = ops.clone();
        for (id, bytes) in final_state(&ops) {
            combined.push((id, bytes));
        }
        let buf_replay = build_via_upserts(generation, &combined);
        prop_assert_eq!(buf_once, buf_replay);
    }

    /// Order-independence at the FINAL-STATE level: any permutation of
    /// the per-doc last-write-wins state yields the same CBOR encoding.
    #[test]
    fn same_final_state_same_bytes(
        generation in 1u64..=10_000u64,
        ops in ops_strategy(),
    ) {
        let state = final_state(&ops);
        // Build A: apply ops as given.
        let buf_a = build_via_upserts(generation, &ops);
        // Build B: apply only the final per-doc state, in doc-id order.
        let canonical: Vec<(u64, Vec<u8>)> =
            state.into_iter().collect();
        let buf_b = build_via_upserts(generation, &canonical);
        prop_assert_eq!(buf_a, buf_b);
    }
}
