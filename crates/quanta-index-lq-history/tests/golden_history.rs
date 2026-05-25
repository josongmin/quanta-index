//! Golden snapshot for the LEX-07 history filter primitives.
//!
//! Builds an 8-commit graph (linear + fork + merge) with stable SHAs and
//! exercises every filter primitive against pinned expected results. Any
//! drift here is a wire-level regression.

use quanta_index_lq_history::types::{AppliedAtMs, CommitSha};
use quanta_index_lq_history::{
    CommitGraph, CommitNode, RevisionRange, enumerate_revisions, merge_commits,
    parents_within_depth, since_time, tag_resolve,
};

fn sha(byte: u8) -> CommitSha {
    CommitSha::from_bytes([byte; 20])
}

/// Strict-mode upsert helper that fails the test on a typed error.
fn ups(g: &mut CommitGraph, node: CommitNode) {
    if let Err(e) = g.upsert_commit(node) {
        assert!(false, "upsert_commit: {e}");
    }
}

/// Build the canonical fixture graph:
///
/// ```text
///   1 -- 2 -- 3 -- 4 ----\
///    \                    7 -- 8
///     5 -- 6 ------------/
/// ```
///
/// SHA bytes match the commit number, so debugging is straightforward.
fn fixture() -> CommitGraph {
    let mut g = CommitGraph::new();
    // 1: root
    ups(
        &mut g,
        CommitNode::new(sha(1), Vec::new(), AppliedAtMs::new(100)),
    );
    // 2..4: linear main
    ups(
        &mut g,
        CommitNode::new(sha(2), vec![sha(1)], AppliedAtMs::new(200)),
    );
    ups(
        &mut g,
        CommitNode::new(sha(3), vec![sha(2)], AppliedAtMs::new(300)),
    );
    ups(
        &mut g,
        CommitNode::new(sha(4), vec![sha(3)], AppliedAtMs::new(400)),
    );
    // 5..6: feature branch off sha(1)
    ups(
        &mut g,
        CommitNode::new(sha(5), vec![sha(1)], AppliedAtMs::new(500)),
    );
    ups(
        &mut g,
        CommitNode::new(sha(6), vec![sha(5)], AppliedAtMs::new(600)),
    );
    // 7: merge of 4 and 6
    ups(
        &mut g,
        CommitNode::new(sha(7), vec![sha(4), sha(6)], AppliedAtMs::new(700)),
    );
    // 8: child of merge
    ups(
        &mut g,
        CommitNode::new(sha(8), vec![sha(7)], AppliedAtMs::new(800)),
    );
    // Refs and tags
    let _prev_ref: Option<CommitSha> = g.add_ref("main", sha(8));
    let _prev_tag: Option<CommitSha> = g.add_tag("v1.0", sha(4));
    let _prev_tag2: Option<CommitSha> = g.add_tag("release", sha(7));
    g
}

#[test]
fn golden_parent_walk_main() {
    let g = fixture();
    let got = match parents_within_depth(&g, &sha(8), 64) {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    // All 8 commits reachable.
    assert_eq!(got.len(), 8);
    let mut i: u8 = 1;
    while i <= 8 {
        assert!(got.contains(&sha(i)), "missing sha({i})");
        i = i.saturating_add(1);
    }
}

#[test]
fn golden_parent_walk_depth_capped() {
    let g = fixture();
    // From sha(8): depth 0 -> {8}, depth 1 adds {7}, depth 2 adds {4,6},
    // depth 3 adds {3,5}. Cap at 3 means commits at depth 4 (sha 2) would
    // push past -> reject.
    match parents_within_depth(&g, &sha(8), 3) {
        Ok(_) => assert!(false, "must fail at depth 4"),
        Err(e) => assert_eq!(
            e.code,
            quanta_index_lq_history::HistoryErrorCode::PlanLimitExceeded
        ),
    }
}

#[test]
fn golden_merge_commits() {
    let g = fixture();
    let got = merge_commits(&g);
    assert_eq!(got, vec![sha(7)]);
}

#[test]
fn golden_tag_exact() {
    let g = fixture();
    let got = match tag_resolve(&g, "v1.0") {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(got.len(), 1);
    let Some((name, s)) = got.first() else {
        assert!(false, "expected pair");
        return;
    };
    assert_eq!(name.as_ref(), "v1.0");
    assert_eq!(*s, sha(4));
}

#[test]
fn golden_revisions_two_dot() {
    let g = fixture();
    // 1..8 -> ancestors(8) \ ancestors(1) = {2,3,4,5,6,7,8}
    let got = match enumerate_revisions(
        &g,
        &RevisionRange::TwoDot {
            a: sha(1),
            b: sha(8),
        },
        100,
    ) {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(
        got,
        vec![sha(2), sha(3), sha(4), sha(5), sha(6), sha(7), sha(8)]
    );
}

#[test]
fn golden_revisions_three_dot_branches() {
    let g = fixture();
    // 4...6 -> sym diff of ancestors({1,2,3,4}) and ancestors({1,5,6})
    //        = {2,3,4} sym {5,6} = {2,3,4,5,6}
    let got = match enumerate_revisions(
        &g,
        &RevisionRange::ThreeDot {
            a: sha(4),
            b: sha(6),
        },
        100,
    ) {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(got, vec![sha(2), sha(3), sha(4), sha(5), sha(6)]);
}

#[test]
fn golden_since_time_inclusive() {
    let g = fixture();
    // threshold 500ms -> commits 5,6,7,8 (applied_at_ms = 500,600,700,800)
    let got = since_time(&g, AppliedAtMs::new(500));
    assert_eq!(got, vec![sha(5), sha(6), sha(7), sha(8)]);
}
