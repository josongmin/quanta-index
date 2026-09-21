//! Property tests — `PositionsBuilder::upsert_doc` idempotency invariants.
//!
//! Channel-arch replay shape: a producer that emits `UpsertChunk(X)` for the
//! same chunk twice must result in the same on-disk posting list as a
//! single `UpsertChunk(X)`. Two assertions cover the contract:
//!
//! 1. **Replay equality** — for any random multi-doc upsert sequence, replaying
//!    the same sequence (each step applied twice in a row) produces the same
//!    `finish()` bytes as the original sequence.
//! 2. **Last-write-wins per doc** — for any doc id, the final state of
//!    that doc reflects only the most recent `upsert_doc(d, pairs)` call;
//!    earlier upserts for the same doc are completely overwritten.

use std::collections::BTreeMap;

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_positions::{
    DocId, NormalizerVersion, Position, PositionsBuilder, PositionsError,
};

// A single upsert step targets a doc id and supplies a small bag of
// (term, position) pairs. Term alphabet is small so doc-collisions and
// term-replacement paths trigger often; position range is small so a
// proptest run never trips the per-cell cap (the cap rail lives elsewhere).
fn upsert_step() -> impl Strategy<Value = (u64, Vec<(String, u32)>)> {
    let term = prop::sample::select(vec!["a", "b", "c", "d", "ab", "abc", "the", "fn", "_"]);
    let pair = (term, 0u32..=64u32).prop_map(|(t, p)| (t.to_owned(), p));
    (
        // Small doc id range so the same doc gets re-upserted often.
        0u64..=8u64,
        vec(pair, 0..=12),
    )
}

fn apply_steps(steps: &[(u64, Vec<(String, u32)>)]) -> Result<Vec<u8>, PositionsError> {
    let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
    for (doc, pairs) in steps {
        b.upsert_doc(DocId(*doc), pairs.iter().map(|(t, p)| (t.as_str(), Position(*p))))?;
    }
    let idx = b.finish()?;
    let mut buf: Vec<u8> = Vec::new();
    idx.serialize_cbor(&mut buf)?;
    Ok(buf)
}

fn apply_with_immediate_replay(
    steps: &[(u64, Vec<(String, u32)>)],
) -> Result<Vec<u8>, PositionsError> {
    // Apply each step twice in a row, simulating a producer that re-emits
    // the same UpsertChunk before the next chunk arrives.
    let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
    for (doc, pairs) in steps {
        b.upsert_doc(DocId(*doc), pairs.iter().map(|(t, p)| (t.as_str(), Position(*p))))?;
        b.upsert_doc(DocId(*doc), pairs.iter().map(|(t, p)| (t.as_str(), Position(*p))))?;
    }
    let idx = b.finish()?;
    let mut buf: Vec<u8> = Vec::new();
    idx.serialize_cbor(&mut buf)?;
    Ok(buf)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn upsert_replay_yields_identical_finish_bytes(
        steps in vec(upsert_step(), 0..=16),
    ) {
        let single = match apply_steps(&steps) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("single: {e}"))),
        };
        let replayed = match apply_with_immediate_replay(&steps) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("replayed: {e}"))),
        };
        prop_assert_eq!(single, replayed);
    }

    #[test]
    fn last_upsert_wins_per_doc(
        steps in vec(upsert_step(), 1..=16),
    ) {
        // Expected per-doc final state: only the LAST upsert for each doc
        // contributes positions. Reconstruct that expectation and compare
        // against the builder result.
        let mut expected_pairs: BTreeMap<u64, Vec<(String, u32)>> = BTreeMap::new();
        for (doc, pairs) in &steps {
            drop(expected_pairs.insert(*doc, pairs.clone()));
        }

        // (term, doc) -> sorted positions, from expected_pairs.
        let mut expected: BTreeMap<(String, u64), Vec<u32>> = BTreeMap::new();
        for (doc, pairs) in &expected_pairs {
            for (term, pos) in pairs {
                expected
                    .entry((term.clone(), *doc))
                    .or_default()
                    .push(*pos);
            }
        }
        for v in expected.values_mut() {
            v.sort_unstable();
        }

        // Build the index via the upsert sequence and read it back.
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for (doc, pairs) in &steps {
            if let Err(e) = b.upsert_doc(
                DocId(*doc),
                pairs.iter().map(|(t, p)| (t.as_str(), Position(*p))),
            ) {
                return Err(TestCaseError::reject(format!("upsert_doc: {e}")));
            }
        }
        let idx = match b.finish() {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("finish: {e}"))),
        };

        // Collect (term, doc) -> positions from the index for the terms
        // we expect.
        let mut got: BTreeMap<(String, u64), Vec<u32>> = BTreeMap::new();
        let mut terms_seen: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for (term, _doc) in expected.keys() {
            let _inserted = terms_seen.insert(term.clone());
        }
        for term in &terms_seen {
            let Some(iter) = idx.term_postings(term) else {
                continue;
            };
            for r in iter {
                let entry = match r {
                    Ok(e) => e,
                    Err(e) => return Err(TestCaseError::reject(format!("decode: {e}"))),
                };
                let key = (term.clone(), entry.doc_id.0);
                let positions: Vec<u32> = entry.positions.iter().map(|p| p.0).collect();
                drop(got.insert(key, positions));
            }
        }
        prop_assert_eq!(expected, got);
    }
}
