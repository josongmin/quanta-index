//! Property test: arbitrary commit/ref/tag op sequence replayed
//! produces a byte-identical CBOR encoding.
//!
//! This is the delta-semantics regression guard. Even with overwrite,
//! reapplication of the same op sequence must converge on the same
//! graph state and the same wire bytes. 256 cases per
//! `tools/ci/proptest/cases.toml`.

use proptest::prelude::*;

use quanta_index_lq_history::types::{AppliedAtMs, CommitSha};
use quanta_index_lq_history::{CommitGraph, CommitNode};

#[derive(Clone, Debug)]
enum Op {
    Upsert {
        sha: CommitSha,
        parents: Vec<CommitSha>,
        applied_at_ms: AppliedAtMs,
    },
    AddTag {
        name: String,
        sha: CommitSha,
    },
    AddRef {
        name: String,
        sha: CommitSha,
    },
}

fn arb_sha() -> impl Strategy<Value = CommitSha> {
    any::<[u8; 20]>().prop_map(CommitSha::from_bytes)
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (
            arb_sha(),
            prop::collection::vec(arb_sha(), 0..3),
            any::<u64>()
        )
            .prop_map(|(sha, parents, t)| Op::Upsert {
                sha,
                parents,
                applied_at_ms: AppliedAtMs::new(t),
            }),
        (".*", arb_sha()).prop_map(|(name, sha)| Op::AddTag { name, sha }),
        (".*", arb_sha()).prop_map(|(name, sha)| Op::AddRef { name, sha }),
    ]
}

fn apply_op(g: &mut CommitGraph, op: &Op) {
    match op {
        Op::Upsert {
            sha,
            parents,
            applied_at_ms,
        } => {
            let node = CommitNode::new(*sha, parents.clone(), *applied_at_ms);
            // Buffered mode tolerates random parent ordering.
            let res = g.upsert_commit(node);
            let _prior: Option<CommitNode> = match res {
                Ok(p) => p,
                Err(err) => {
                    let _bound: String = err.to_string();
                    None
                }
            };
        }
        Op::AddTag { name, sha } => {
            let _prev: Option<CommitSha> = g.add_tag(name.as_str(), *sha);
        }
        Op::AddRef { name, sha } => {
            let _prev: Option<CommitSha> = g.add_ref(name.as_str(), *sha);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Replaying the same op sequence twice (against two fresh graphs)
    /// yields byte-identical CBOR. Idempotency + determinism guard.
    #[test]
    fn upsert_sequence_replay_is_byte_identical(ops in prop::collection::vec(arb_op(), 0..24)) {
        let mut g_a = CommitGraph::new().with_buffering();
        let mut g_b = CommitGraph::new().with_buffering();
        for op in &ops {
            apply_op(&mut g_a, op);
            apply_op(&mut g_b, op);
        }
        prop_assert_eq!(&g_a, &g_b);

        let mut bytes_a: Vec<u8> = Vec::new();
        let mut bytes_b: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(&g_a, &mut bytes_a)
            .map_err(|e| TestCaseError::fail(format!("ser g_a: {e}")))?;
        ciborium::ser::into_writer(&g_b, &mut bytes_b)
            .map_err(|e| TestCaseError::fail(format!("ser g_b: {e}")))?;
        prop_assert_eq!(bytes_a, bytes_b);
    }

    /// Appending the same op to a graph twice is idempotent on the wire:
    /// the second application returns the same bytes as the first.
    #[test]
    fn second_application_of_same_ops_is_idempotent(ops in prop::collection::vec(arb_op(), 0..16)) {
        let mut g = CommitGraph::new().with_buffering();
        for op in &ops {
            apply_op(&mut g, op);
        }
        let mut bytes_once: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(&g, &mut bytes_once)
            .map_err(|e| TestCaseError::fail(format!("ser once: {e}")))?;

        // Apply every op a second time.
        for op in &ops {
            apply_op(&mut g, op);
        }
        let mut bytes_twice: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(&g, &mut bytes_twice)
            .map_err(|e| TestCaseError::fail(format!("ser twice: {e}")))?;
        prop_assert_eq!(bytes_once, bytes_twice);
    }
}
