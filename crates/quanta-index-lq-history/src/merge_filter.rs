//! `merge:` filter primitive.
//!
//! [`merge_commits`] enumerates every commit in the graph with two or more
//! parents, sorted ascending by sha. Per Q-LEX07-5 the merge filter targets
//! the merge result diff itself, never the merged-in side.

use crate::commit_graph::CommitGraph;
use crate::types::CommitSha;

/// All commits with two or more parents, in ascending sha order.
#[must_use]
pub fn merge_commits(graph: &CommitGraph) -> Vec<CommitSha> {
    graph
        .nodes()
        .filter(|n| n.is_merge())
        .map(|n| *n.sha())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::merge_commits;
    use crate::commit_graph::{CommitGraph, CommitNode};
    use crate::types::{AppliedAtMs, CommitSha};

    fn sha(byte: u8) -> CommitSha {
        CommitSha::from_bytes([byte; 20])
    }

    #[test]
    fn empty_graph_returns_empty() {
        let g = CommitGraph::new();
        assert!(merge_commits(&g).is_empty());
    }

    #[test]
    fn graph_with_no_merges_returns_empty() {
        let mut g = CommitGraph::new();
        g.add_commit(CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0)));
        g.add_commit(CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1)));
        g.add_commit(CommitNode::new(sha(2), vec![sha(1)], AppliedAtMs::new(2)));
        assert!(merge_commits(&g).is_empty());
    }

    #[test]
    fn graph_with_one_merge() {
        let mut g = CommitGraph::new();
        g.add_commit(CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0)));
        g.add_commit(CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1)));
        g.add_commit(CommitNode::new(sha(2), vec![sha(0)], AppliedAtMs::new(2)));
        g.add_commit(CommitNode::new(
            sha(3),
            vec![sha(1), sha(2)],
            AppliedAtMs::new(3),
        ));
        let got = merge_commits(&g);
        assert_eq!(got, vec![sha(3)]);
    }

    #[test]
    fn graph_with_multiple_merges_sorted() {
        let mut g = CommitGraph::new();
        g.add_commit(CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0)));
        g.add_commit(CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1)));
        g.add_commit(CommitNode::new(sha(2), vec![sha(0)], AppliedAtMs::new(2)));
        // Two merges intentionally inserted in non-ascending sha order to
        // exercise sorted output.
        g.add_commit(CommitNode::new(
            sha(9),
            vec![sha(1), sha(2)],
            AppliedAtMs::new(9),
        ));
        g.add_commit(CommitNode::new(
            sha(5),
            vec![sha(1), sha(2), sha(0)],
            AppliedAtMs::new(5),
        ));
        let got = merge_commits(&g);
        // CommitGraph stores nodes in BTreeMap order, so sha(5) precedes sha(9).
        assert_eq!(got, vec![sha(5), sha(9)]);
    }
}
