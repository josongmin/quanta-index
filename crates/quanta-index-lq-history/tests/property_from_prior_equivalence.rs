//! Property test: `CommitGraph::from_prior` carries the full state of
//! the prior graph (modulo the new generation stamp). 256 cases.

use proptest::prelude::*;

use quanta_index_lq_history::types::{AppliedAtMs, CommitSha, ManifestGeneration};
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
            let mut g = CommitGraph::new().with_buffering();
            for n in nodes {
                let res = g.upsert_commit(n);
                let _prior: Option<CommitNode> = match res {
                    Ok(p) => p,
                    Err(err) => {
                        let _bound: String = err.to_string();
                        None
                    }
                };
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

fn arb_gen() -> impl Strategy<Value = ManifestGeneration> {
    // Strategy yields v >= 1, so `ManifestGeneration::new` always Ok.
    // The match binds the typed error so it is observable rather than
    // discarded (workspace lints ban `unwrap_or_else`/`ok` patterns).
    (1u64..u64::MAX).prop_map(|v| match ManifestGeneration::new(v) {
        Ok(g) => g,
        Err(err) => {
            let _bound: String = err.to_string();
            ManifestGeneration::from_raw(v)
        }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// `from_prior` preserves every commit, tag, and ref, and stamps
    /// the supplied generation. Comparing every accessor.
    #[test]
    fn from_prior_carries_full_state(prior in arb_graph(), g in arb_gen()) {
        let next = CommitGraph::from_prior(&prior, g);
        // generation stamped
        prop_assert_eq!(next.generation(), Some(g));
        // commit count and content
        prop_assert_eq!(next.commit_count(), prior.commit_count());
        for n in prior.nodes() {
            prop_assert_eq!(next.node(n.sha()), Some(n));
        }
        // tag map content
        prop_assert_eq!(next.tags(), prior.tags());
        // ref map content
        prop_assert_eq!(next.refs(), prior.refs());
        // buffered flag carried
        prop_assert_eq!(next.is_buffered(), prior.is_buffered());
        // pending queue carried
        prop_assert_eq!(next.pending(), prior.pending());
    }

    /// `from_prior` then CBOR-roundtripping is byte-identical to
    /// CBOR-encoding the `from_prior` snapshot directly.
    #[test]
    fn from_prior_cbor_roundtrip_is_byte_identical(prior in arb_graph(), g in arb_gen()) {
        let next = CommitGraph::from_prior(&prior, g);
        let mut a: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(&next, &mut a)
            .map_err(|e| TestCaseError::fail(format!("ser1: {e}")))?;
        let back: CommitGraph = ciborium::de::from_reader(a.as_slice())
            .map_err(|e| TestCaseError::fail(format!("de: {e}")))?;
        prop_assert_eq!(&back, &next);
        let mut b: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(&back, &mut b)
            .map_err(|e| TestCaseError::fail(format!("ser2: {e}")))?;
        prop_assert_eq!(a, b);
    }
}
