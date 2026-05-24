//! Property test: arbitrary `CommitGraph` survives CBOR roundtrip byte-
//! identically. 256 cases enforces enough coverage to catch any field-
//! ordering or encoding regression in the manual serde impls.

use proptest::prelude::*;

use quanta_index_lq_history::types::{AppliedAtMs, CommitSha};
use quanta_index_lq_history::{CommitGraph, CommitNode};

fn arb_sha() -> impl Strategy<Value = CommitSha> {
    any::<[u8; 20]>().prop_map(CommitSha::from_bytes)
}

fn arb_node() -> impl Strategy<Value = CommitNode> {
    (
        arb_sha(),
        prop::collection::vec(arb_sha(), 0..3),
        any::<u64>(),
    )
        .prop_map(|(s, ps, t)| CommitNode::new(s, ps, AppliedAtMs::new(t)))
}

fn arb_graph() -> impl Strategy<Value = CommitGraph> {
    (
        prop::collection::vec(arb_node(), 0..16),
        prop::collection::vec((".*", arb_sha()), 0..4),
        prop::collection::vec((".*", arb_sha()), 0..4),
    )
        .prop_map(|(nodes, tags, refs)| {
            let mut g = CommitGraph::new();
            for n in nodes {
                g.add_commit(n);
            }
            for (name, sha) in tags {
                let _prev: Option<CommitSha> = g.add_tag(name, sha);
            }
            for (name, sha) in refs {
                let _prev: Option<CommitSha> = g.add_ref(name, sha);
            }
            g
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn cbor_roundtrip_is_byte_identical(g in arb_graph()) {
        let mut a: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(&g, &mut a).map_err(|e| TestCaseError::fail(format!("ser1: {e}")))?;
        let back: CommitGraph = ciborium::de::from_reader(a.as_slice())
            .map_err(|e| TestCaseError::fail(format!("de: {e}")))?;
        prop_assert_eq!(&back, &g);
        // Re-serialize and demand byte-identical bytes.
        let mut b: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(&back, &mut b).map_err(|e| TestCaseError::fail(format!("ser2: {e}")))?;
        prop_assert_eq!(a, b);
    }
}
