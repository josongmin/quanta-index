//! Depth-capped parent walk over the commit DAG.
//!
//! [`parents_within_depth`] performs a breadth-first walk starting at `root`,
//! returning every ancestor commit reachable in at most `max_depth` parent
//! hops. The returned set includes the root commit itself (depth 0).
//!
//! Failure modes:
//!
//! - root not in the graph -> [`HistoryErrorCode::HistoryRefNotFound`]
//! - any parent edge that closes a cycle within the reachable subgraph ->
//!   [`HistoryErrorCode::HistoryMergeCycle`]
//! - any commit observed at depth strictly greater than `max_depth` ->
//!   [`HistoryErrorCode::PlanLimitExceeded`] with
//!   [`LimitDimension::ParentDepth`]
//!
//! Implementation: a DFS pre-pass over reachable parents detects cycles via
//! gray/black coloring; the BFS then collects ancestors with a strict depth
//! cap. Both passes are fail-closed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::commit_graph::CommitGraph;
use crate::errors::{HistoryError, HistoryErrorCode, LimitDimension};
use crate::types::CommitSha;

/// Breadth-first walk from `root` over the parent edges, depth-capped.
///
/// The returned set is `BTreeSet`-ordered. Includes `root` itself.
pub fn parents_within_depth(
    graph: &CommitGraph,
    root: &CommitSha,
    max_depth: u32,
) -> Result<BTreeSet<CommitSha>, HistoryError> {
    if graph.node(root).is_none() {
        return Err(HistoryError::new(
            HistoryErrorCode::HistoryRefNotFound,
            format!("parent: root commit {root} not found in graph"),
        ));
    }

    // Pre-pass: gray/black DFS detects cycles in the reachable subgraph
    // and surfaces any dangling parent edge as HistoryRefNotFound.
    detect_cycles(graph, root)?;

    // BFS collects depth-capped ancestors.
    let mut visited: BTreeSet<CommitSha> = BTreeSet::new();
    let mut queue: VecDeque<(CommitSha, u32)> = VecDeque::new();
    let _inserted_root: bool = visited.insert(*root);
    queue.push_back((*root, 0));

    while let Some((sha, depth)) = queue.pop_front() {
        let Some(node) = graph.node(&sha) else {
            // Cycle pre-pass already validated reachability; reaching here
            // means the graph was mutated mid-walk which we refuse.
            return Err(HistoryError::new(
                HistoryErrorCode::HistoryRefNotFound,
                format!("parent: ancestor commit {sha} referenced but absent from graph"),
            ));
        };

        for parent in node.parents() {
            if visited.contains(parent) {
                continue;
            }
            let next_depth = depth.checked_add(1).ok_or_else(|| {
                HistoryError::plan_limit(
                    LimitDimension::ParentDepth,
                    "parent: depth counter overflowed u32",
                )
            })?;
            if next_depth > max_depth {
                return Err(HistoryError::plan_limit(
                    LimitDimension::ParentDepth,
                    format!(
                        "parent: walk depth {next_depth} exceeds cap {max_depth} at commit {parent}"
                    ),
                ));
            }
            let _inserted: bool = visited.insert(*parent);
            queue.push_back((*parent, next_depth));
        }
    }

    Ok(visited)
}

/// Color states used by [`detect_cycles`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    Gray,
    Black,
}

/// DFS gray/black coloring over the reachable subgraph rooted at `root`.
///
/// Surfaces `HistoryMergeCycle` when a parent edge points to a node that is
/// still on the current DFS stack, and `HistoryRefNotFound` when a parent
/// edge points to a sha not present in the graph.
fn detect_cycles(graph: &CommitGraph, root: &CommitSha) -> Result<(), HistoryError> {
    let mut color: BTreeMap<CommitSha, Color> = BTreeMap::new();
    let mut stack: Vec<(CommitSha, usize)> = Vec::new();
    let _prev_color: Option<Color> = color.insert(*root, Color::Gray);
    stack.push((*root, 0));

    while let Some((sha, idx)) = stack.pop() {
        let Some(node) = graph.node(&sha) else {
            return Err(HistoryError::new(
                HistoryErrorCode::HistoryRefNotFound,
                format!("parent: ancestor commit {sha} referenced but absent from graph"),
            ));
        };
        let parents = node.parents();
        if idx >= parents.len() {
            // Done with this node: mark black.
            let _prev: Option<Color> = color.insert(sha, Color::Black);
            continue;
        }
        let parent = match parents.get(idx) {
            Some(p) => *p,
            None => {
                return Err(HistoryError::new(
                    HistoryErrorCode::HistoryMergeCycle,
                    "parent: index out of range during DFS",
                ));
            }
        };
        // Push the resume frame for this node.
        let next_idx = idx.checked_add(1).ok_or_else(|| {
            HistoryError::new(
                HistoryErrorCode::HistoryMergeCycle,
                "parent: parent index overflowed",
            )
        })?;
        stack.push((sha, next_idx));

        match color.get(&parent).copied() {
            Some(Color::Gray) => {
                return Err(HistoryError::new(
                    HistoryErrorCode::HistoryMergeCycle,
                    format!(
                        "parent: cycle detected; parent {parent} of {sha} is still on the DFS stack"
                    ),
                ));
            }
            Some(Color::Black) => {
                // Already fully explored; legitimate diamond. Skip.
            }
            None => {
                let _prev: Option<Color> = color.insert(parent, Color::Gray);
                stack.push((parent, 0));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parents_within_depth;
    use crate::commit_graph::{CommitGraph, CommitNode};
    use crate::errors::{HistoryErrorCode, LimitDimension};
    use crate::types::{AppliedAtMs, CommitSha};

    fn sha(byte: u8) -> CommitSha {
        CommitSha::from_bytes([byte; 20])
    }

    fn linear_chain(len: u8) -> CommitGraph {
        let mut g = CommitGraph::new();
        let mut i: u8 = 0;
        while i < len {
            let parents = if i == 0 {
                Vec::new()
            } else {
                let prev = i.saturating_sub(1);
                vec![sha(prev)]
            };
            g.add_commit(CommitNode::new(
                sha(i),
                parents,
                AppliedAtMs::new(u64::from(i)),
            ));
            i = i.saturating_add(1);
        }
        g
    }

    #[test]
    fn linear_chain_collects_all_ancestors() {
        let g = linear_chain(5);
        let got = match parents_within_depth(&g, &sha(4), 10) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got.len(), 5);
        let mut i: u8 = 0;
        while i < 5 {
            assert!(got.contains(&sha(i)));
            i = i.saturating_add(1);
        }
    }

    #[test]
    fn root_only_when_root_has_no_parents() {
        let g = linear_chain(1);
        let got = match parents_within_depth(&g, &sha(0), 0) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got.len(), 1);
        assert!(got.contains(&sha(0)));
    }

    #[test]
    fn fork_then_merge_diamond() {
        // 0 <- 1 <- 3
        //   \- 2 -/
        let mut g = CommitGraph::new();
        g.add_commit(CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0)));
        g.add_commit(CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1)));
        g.add_commit(CommitNode::new(sha(2), vec![sha(0)], AppliedAtMs::new(2)));
        g.add_commit(CommitNode::new(
            sha(3),
            vec![sha(1), sha(2)],
            AppliedAtMs::new(3),
        ));
        let got = match parents_within_depth(&g, &sha(3), 5) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got.len(), 4);
        let mut i: u8 = 0;
        while i <= 3 {
            assert!(got.contains(&sha(i)));
            i = i.saturating_add(1);
        }
    }

    #[test]
    fn deep_chain_rejects_when_exceeds_cap() {
        let g = linear_chain(10);
        match parents_within_depth(&g, &sha(9), 3) {
            Ok(_) => assert!(false, "must fail at depth cap"),
            Err(e) => {
                assert_eq!(e.code, HistoryErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::ParentDepth));
            }
        }
    }

    #[test]
    fn unknown_root_fails_closed() {
        let g = linear_chain(3);
        match parents_within_depth(&g, &sha(99), 5) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn contrived_self_loop_is_cycle() {
        let mut g = CommitGraph::new();
        g.add_commit(CommitNode::new(sha(0), vec![sha(0)], AppliedAtMs::new(0)));
        match parents_within_depth(&g, &sha(0), 5) {
            Ok(_) => assert!(false, "self-loop must surface cycle"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryMergeCycle),
        }
    }

    #[test]
    fn contrived_two_node_cycle() {
        let mut g = CommitGraph::new();
        g.add_commit(CommitNode::new(sha(0), vec![sha(1)], AppliedAtMs::new(0)));
        g.add_commit(CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1)));
        match parents_within_depth(&g, &sha(0), 5) {
            Ok(_) => assert!(false, "cycle must be detected"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryMergeCycle),
        }
    }

    #[test]
    fn dangling_parent_surfaces_typed_error() {
        let mut g = CommitGraph::new();
        g.add_commit(CommitNode::new(sha(0), vec![sha(7)], AppliedAtMs::new(0)));
        match parents_within_depth(&g, &sha(0), 5) {
            Ok(_) => assert!(false, "dangling parent must surface"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }
}
