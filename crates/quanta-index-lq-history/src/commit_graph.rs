//! Commit DAG cache used by the history filter primitives.
//!
//! [`CommitGraph`] holds every commit node along with two name-to-sha maps:
//! `refs` (branches and arbitrary named refs) and `by_tag` (tag refs).
//! The graph is the only authoritative source consulted by `parent:`,
//! `merge:`, `tag:`, `revisions:`, and `since.time:`; there is no
//! request-time `git log` fallback.
//!
//! Determinism: keys are `BTreeMap`-ordered, so CBOR serialization is
//! byte-identical for the same insertion set.
//!
//! ## Delta semantics (§3.6 incremental boundary)
//!
//! - [`CommitGraph::upsert_commit`] is the single mutator for commit
//!   nodes. It is idempotent on `(sha, parents, applied_at_ms)`: same
//!   node replayed yields the same graph. When the prior node at the
//!   same SHA had a different `parents` set, the prior node is returned
//!   and any downstream-derived state at the call site that indexes
//!   commits by-parent must be invalidated. This crate currently keeps
//!   NO by-parent cache; if one is added in the future (the LEX-07 §12
//!   roadmap calls one out) the invalidation point lives here.
//! - [`CommitGraph::remove_commit`] is provided for search-side rebuild
//!   scenarios. AMB-PROD-3 says the producer never emits a `DeleteCommit`
//!   op — force-push is handled by fresh-generation rebuild — but the
//!   search side may still need to drop a commit during prior-graph
//!   reuse. The policy is **conservative refuse**: a commit with any
//!   in-graph children is refused with [`HistoryErrorCode::HistoryCommitHasChildren`]
//!   rather than silently orphan-rooting the children.
//! - [`CommitGraph::from_prior`] carries every `by_sha` / `by_tag` /
//!   `refs` entry from a prior graph and stamps a fresh generation. This
//!   is the search-side hook for AMB-PROD-3 fresh-generation rebuild.
//! - [`CommitGraph::with_buffering`] flips the upsert policy from
//!   strict (parent-must-exist, fail-closed with
//!   [`HistoryErrorCode::HistoryCommitParentUnknown`]) to buffered
//!   (parent-not-yet-present is recorded in a pending queue and
//!   resolves when the parent arrives). Addresses AMB-PROD-1 producer
//!   commit emission ordering.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use crate::errors::{HistoryError, HistoryErrorCode};
use crate::types::{AppliedAtMs, CommitSha, ManifestGeneration};

/// A single commit node in the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitNode {
    sha: CommitSha,
    parents: Vec<CommitSha>,
    applied_at_ms: AppliedAtMs,
}

impl CommitNode {
    /// Build a new node.
    #[must_use]
    pub fn new(sha: CommitSha, parents: Vec<CommitSha>, applied_at_ms: AppliedAtMs) -> Self {
        Self {
            sha,
            parents,
            applied_at_ms,
        }
    }

    /// Commit SHA of this node.
    #[must_use]
    pub const fn sha(&self) -> &CommitSha {
        &self.sha
    }

    /// Borrowed parent list (zero, one, or more).
    #[must_use]
    pub fn parents(&self) -> &[CommitSha] {
        &self.parents
    }

    /// Apply timestamp recorded by the write-packet trace.
    #[must_use]
    pub const fn applied_at_ms(&self) -> AppliedAtMs {
        self.applied_at_ms
    }

    /// `true` when this commit has two or more parents.
    #[must_use]
    pub fn is_merge(&self) -> bool {
        self.parents.len() >= 2
    }
}

impl serde::Serialize for CommitNode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("CommitNode", 3)?;
        st.serialize_field("sha", &self.sha)?;
        st.serialize_field("parents", &self.parents)?;
        st.serialize_field("applied_at_ms", &self.applied_at_ms)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for CommitNode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            Sha,
            Parents,
            AppliedAtMs,
        }

        impl<'de2> serde::Deserialize<'de2> for Field {
            fn deserialize<D2>(de: D2) -> Result<Self, D2::Error>
            where
                D2: serde::Deserializer<'de2>,
            {
                struct V;
                impl serde::de::Visitor<'_> for V {
                    type Value = Field;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("CommitNode field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "sha" => Ok(Field::Sha),
                            "parents" => Ok(Field::Parents),
                            "applied_at_ms" => Ok(Field::AppliedAtMs),
                            other => Err(E::unknown_field(
                                other,
                                &["sha", "parents", "applied_at_ms"],
                            )),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct NodeVisitor;
        impl<'d> serde::de::Visitor<'d> for NodeVisitor {
            type Value = CommitNode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("CommitNode struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<CommitNode, A::Error> {
                let mut sha: Option<CommitSha> = None;
                let mut parents: Option<Vec<CommitSha>> = None;
                let mut applied_at_ms: Option<AppliedAtMs> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::Sha => {
                            if sha.is_some() {
                                return Err(serde::de::Error::duplicate_field("sha"));
                            }
                            sha = Some(map.next_value()?);
                        }
                        Field::Parents => {
                            if parents.is_some() {
                                return Err(serde::de::Error::duplicate_field("parents"));
                            }
                            parents = Some(map.next_value()?);
                        }
                        Field::AppliedAtMs => {
                            if applied_at_ms.is_some() {
                                return Err(serde::de::Error::duplicate_field("applied_at_ms"));
                            }
                            applied_at_ms = Some(map.next_value()?);
                        }
                    }
                }
                let sha = sha.ok_or_else(|| serde::de::Error::missing_field("sha"))?;
                let parents = parents.ok_or_else(|| serde::de::Error::missing_field("parents"))?;
                let applied_at_ms = applied_at_ms
                    .ok_or_else(|| serde::de::Error::missing_field("applied_at_ms"))?;
                Ok(CommitNode {
                    sha,
                    parents,
                    applied_at_ms,
                })
            }
        }

        de.deserialize_struct(
            "CommitNode",
            &["sha", "parents", "applied_at_ms"],
            NodeVisitor,
        )
    }
}

/// Commit DAG cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitGraph {
    by_sha: BTreeMap<CommitSha, CommitNode>,
    by_tag: BTreeMap<Box<str>, CommitSha>,
    refs: BTreeMap<Box<str>, CommitSha>,
    /// When `true`, [`CommitGraph::upsert_commit`] tolerates a commit
    /// whose parents are not yet in the graph. Such commits are inserted
    /// and their pending parent-resolution is tracked in `pending`.
    /// Default `false` (strict, fail-closed).
    buffered: bool,
    /// Children that referenced an absent parent in buffered mode. Keyed
    /// by the absent parent SHA, value is the set of waiting child SHAs.
    /// When that parent arrives via `upsert_commit`, the children "flip"
    /// to resolved and are removed from the queue.
    pending: BTreeMap<CommitSha, BTreeSet<CommitSha>>,
    /// Optional generation stamp carried by [`CommitGraph::from_prior`].
    /// Search-side rebuild paths use this to associate a graph snapshot
    /// with a manifest generation; producers leave this `None`.
    generation: Option<ManifestGeneration>,
}

impl CommitGraph {
    /// Empty graph in strict (non-buffered) mode.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_sha: BTreeMap::new(),
            by_tag: BTreeMap::new(),
            refs: BTreeMap::new(),
            buffered: false,
            pending: BTreeMap::new(),
            generation: None,
        }
    }

    /// Flip on buffered mode for this graph.
    ///
    /// In buffered mode, [`CommitGraph::upsert_commit`] accepts a node
    /// whose parents are not yet present; the resolution is deferred
    /// and tracked in the pending queue. When the missing parent later
    /// arrives via `upsert_commit`, the child is removed from the
    /// pending queue (resolution "flips").
    ///
    /// Strict mode (the default) surfaces
    /// [`HistoryErrorCode::HistoryCommitParentUnknown`] on the same
    /// input.
    #[must_use]
    pub fn with_buffering(mut self) -> Self {
        self.buffered = true;
        self
    }

    /// Returns `true` when the graph is in buffered mode.
    #[must_use]
    pub const fn is_buffered(&self) -> bool {
        self.buffered
    }

    /// Snapshot of children whose parent is still unresolved. Keyed by
    /// the absent parent SHA; value is the set of pending children.
    ///
    /// Useful for search-side audits when buffered mode is active.
    #[must_use]
    pub const fn pending(&self) -> &BTreeMap<CommitSha, BTreeSet<CommitSha>> {
        &self.pending
    }

    /// Optional generation stamp carried by [`CommitGraph::from_prior`].
    #[must_use]
    pub const fn generation(&self) -> Option<ManifestGeneration> {
        self.generation
    }

    /// Build a fresh graph from `prior`, carrying every `by_sha`,
    /// `by_tag`, and `refs` entry and stamping `new_generation`. The
    /// buffered flag and pending queue are also carried over so a
    /// search-side rebuild on top of a buffered prior keeps its
    /// resolution state.
    ///
    /// This is the search-side rebuild hook for AMB-PROD-3
    /// fresh-generation force-push handling: the producer side emits a
    /// new generation, the search side calls `from_prior` to seed a
    /// new graph from the snapshot it already had, then layers fresh
    /// upserts on top.
    #[must_use]
    pub fn from_prior(prior: &Self, new_generation: ManifestGeneration) -> Self {
        Self {
            by_sha: prior.by_sha.clone(),
            by_tag: prior.by_tag.clone(),
            refs: prior.refs.clone(),
            buffered: prior.buffered,
            pending: prior.pending.clone(),
            generation: Some(new_generation),
        }
    }

    /// Insert or replace a commit node.
    ///
    /// Behavior:
    ///
    /// - If `node.sha` already exists in `by_sha`, the prior node is
    ///   **replaced** and returned. The caller is responsible for
    ///   invalidating any downstream by-parent cache when the prior
    ///   node's `parents` differs from the new node's. This crate keeps
    ///   no such cache; the §12 LEX-07 roadmap calls one out.
    /// - When buffered mode is OFF and any parent in `node.parents()`
    ///   is absent from `by_sha`, the call fails closed with
    ///   [`HistoryErrorCode::HistoryCommitParentUnknown`].
    /// - When buffered mode is ON, an absent parent records the child
    ///   in `pending[parent]`; the child resolves automatically when
    ///   the parent later arrives via `upsert_commit`.
    pub fn upsert_commit(&mut self, node: CommitNode) -> Result<Option<CommitNode>, HistoryError> {
        let sha = *node.sha();
        // Parent validation. A parent referencing the node itself is
        // allowed in strict mode: the inserted SHA equals the parent
        // SHA, treated as "present after this insert"; any cycle
        // surfaces at walk time.
        if self.buffered {
            // Record unresolved parents in the pending queue.
            for parent in node.parents() {
                if *parent == sha {
                    continue;
                }
                if !self.by_sha.contains_key(parent) {
                    let waiting = self.pending.entry(*parent).or_default();
                    let _new: bool = waiting.insert(sha);
                }
            }
        } else {
            for parent in node.parents() {
                if *parent == sha {
                    continue;
                }
                if !self.by_sha.contains_key(parent) {
                    return Err(HistoryError::new(
                        HistoryErrorCode::HistoryCommitParentUnknown,
                        format!(
                            "upsert_commit: parent {parent} of {sha} not in graph (strict mode)"
                        ),
                    ));
                }
            }
        }

        let prior = self.by_sha.insert(sha, node);

        // If buffered: any pending children waiting on this SHA flip to
        // resolved. We drop the queue entry; the child rows already live
        // in by_sha and are now reachable.
        if self.buffered {
            let _resolved: Option<BTreeSet<CommitSha>> = self.pending.remove(&sha);
        }

        Ok(prior)
    }

    /// Remove a commit from the graph.
    ///
    /// Returns the removed node if present.
    ///
    /// Policy: conservative refuse — a commit with any in-graph child
    /// referencing it as parent is refused with
    /// [`HistoryErrorCode::HistoryCommitHasChildren`]. Forcing removal
    /// would orphan-root the children, which the parent walk surfaces
    /// only after the fact as a dangling-edge failure; refusing here
    /// keeps the failure closed at the mutation site.
    ///
    /// Removing a nonexistent SHA returns `Ok(None)` without error,
    /// keeping the operation idempotent for replay scenarios.
    ///
    /// AMB-PROD-3 note: the channel-op `DeleteCommit` is *deliberately
    /// not* synthesized from this call. Force-push handling is
    /// fresh-generation; this API exists for search-side prior-graph
    /// reuse only.
    pub fn remove_commit(&mut self, sha: &CommitSha) -> Result<Option<CommitNode>, HistoryError> {
        if !self.by_sha.contains_key(sha) {
            return Ok(None);
        }
        // Children check: any node whose parents include this sha.
        let mut children: Vec<CommitSha> = Vec::new();
        for (child_sha, node) in &self.by_sha {
            if child_sha == sha {
                continue;
            }
            for parent in node.parents() {
                if parent == sha {
                    children.push(*child_sha);
                    break;
                }
            }
        }
        if !children.is_empty() {
            let count = children.len();
            return Err(HistoryError::new(
                HistoryErrorCode::HistoryCommitHasChildren,
                format!("remove_commit: {sha} has {count} in-graph child commit(s); refuse"),
            ));
        }
        let removed = self.by_sha.remove(sha);
        // Drop any pending-queue entry keyed on this sha. The waiting
        // children remain in by_sha (their lookup will surface as a
        // dangling parent at walk time if they re-enter strict mode).
        let _pending: Option<BTreeSet<CommitSha>> = self.pending.remove(sha);
        Ok(removed)
    }

    /// Insert (or overwrite) a tag → commit mapping. Returns the prior
    /// value when the tag already existed.
    pub fn add_tag(&mut self, name: impl Into<Box<str>>, sha: CommitSha) -> Option<CommitSha> {
        self.by_tag.insert(name.into(), sha)
    }

    /// Insert (or overwrite) a named ref (branch or HEAD-like). Returns
    /// the prior value when the ref already existed.
    pub fn add_ref(&mut self, name: impl Into<Box<str>>, sha: CommitSha) -> Option<CommitSha> {
        self.refs.insert(name.into(), sha)
    }

    /// Lookup commit by sha.
    #[must_use]
    pub fn node(&self, sha: &CommitSha) -> Option<&CommitNode> {
        self.by_sha.get(sha)
    }

    /// Iterate every commit in sorted-sha order.
    pub fn nodes(&self) -> impl Iterator<Item = &CommitNode> {
        self.by_sha.values()
    }

    /// Count of commits in the graph.
    #[must_use]
    pub fn commit_count(&self) -> usize {
        self.by_sha.len()
    }

    /// Tag map view.
    #[must_use]
    pub fn tags(&self) -> &BTreeMap<Box<str>, CommitSha> {
        &self.by_tag
    }

    /// Ref map view.
    #[must_use]
    pub fn refs(&self) -> &BTreeMap<Box<str>, CommitSha> {
        &self.refs
    }

    /// Resolve a named ref (`refs` first, then `by_tag`) to a sha.
    ///
    /// Returns [`HistoryErrorCode::HistoryRefNotFound`] when nothing
    /// matches.
    pub fn resolve_ref(&self, name: &str) -> Result<&CommitSha, HistoryError> {
        if let Some(sha) = self.refs.get(name) {
            return Ok(sha);
        }
        if let Some(sha) = self.by_tag.get(name) {
            return Ok(sha);
        }
        Err(HistoryError::new(
            HistoryErrorCode::HistoryRefNotFound,
            format!("ref `{name}` not found in commit graph"),
        ))
    }
}

impl serde::Serialize for CommitGraph {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("CommitGraph", 6)?;
        st.serialize_field("by_sha", &self.by_sha)?;
        st.serialize_field("by_tag", &self.by_tag)?;
        st.serialize_field("refs", &self.refs)?;
        st.serialize_field("buffered", &self.buffered)?;
        st.serialize_field("pending", &self.pending)?;
        st.serialize_field("generation", &self.generation)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for CommitGraph {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            BySha,
            ByTag,
            Refs,
            Buffered,
            Pending,
            Generation,
        }
        impl<'de2> serde::Deserialize<'de2> for Field {
            fn deserialize<D2>(de: D2) -> Result<Self, D2::Error>
            where
                D2: serde::Deserializer<'de2>,
            {
                struct V;
                impl serde::de::Visitor<'_> for V {
                    type Value = Field;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("CommitGraph field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "by_sha" => Ok(Field::BySha),
                            "by_tag" => Ok(Field::ByTag),
                            "refs" => Ok(Field::Refs),
                            "buffered" => Ok(Field::Buffered),
                            "pending" => Ok(Field::Pending),
                            "generation" => Ok(Field::Generation),
                            other => Err(E::unknown_field(
                                other,
                                &[
                                    "by_sha",
                                    "by_tag",
                                    "refs",
                                    "buffered",
                                    "pending",
                                    "generation",
                                ],
                            )),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct GraphVisitor;
        impl<'d> serde::de::Visitor<'d> for GraphVisitor {
            type Value = CommitGraph;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("CommitGraph struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<CommitGraph, A::Error> {
                let mut by_sha: Option<BTreeMap<CommitSha, CommitNode>> = None;
                let mut by_tag: Option<BTreeMap<Box<str>, CommitSha>> = None;
                let mut refs: Option<BTreeMap<Box<str>, CommitSha>> = None;
                let mut buffered: Option<bool> = None;
                let mut pending: Option<BTreeMap<CommitSha, BTreeSet<CommitSha>>> = None;
                let mut generation: Option<Option<ManifestGeneration>> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::BySha => {
                            if by_sha.is_some() {
                                return Err(serde::de::Error::duplicate_field("by_sha"));
                            }
                            by_sha = Some(map.next_value()?);
                        }
                        Field::ByTag => {
                            if by_tag.is_some() {
                                return Err(serde::de::Error::duplicate_field("by_tag"));
                            }
                            by_tag = Some(map.next_value()?);
                        }
                        Field::Refs => {
                            if refs.is_some() {
                                return Err(serde::de::Error::duplicate_field("refs"));
                            }
                            refs = Some(map.next_value()?);
                        }
                        Field::Buffered => {
                            if buffered.is_some() {
                                return Err(serde::de::Error::duplicate_field("buffered"));
                            }
                            buffered = Some(map.next_value()?);
                        }
                        Field::Pending => {
                            if pending.is_some() {
                                return Err(serde::de::Error::duplicate_field("pending"));
                            }
                            pending = Some(map.next_value()?);
                        }
                        Field::Generation => {
                            if generation.is_some() {
                                return Err(serde::de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                    }
                }
                Ok(CommitGraph {
                    by_sha: by_sha.ok_or_else(|| serde::de::Error::missing_field("by_sha"))?,
                    by_tag: by_tag.ok_or_else(|| serde::de::Error::missing_field("by_tag"))?,
                    refs: refs.ok_or_else(|| serde::de::Error::missing_field("refs"))?,
                    buffered: buffered
                        .ok_or_else(|| serde::de::Error::missing_field("buffered"))?,
                    pending: pending.ok_or_else(|| serde::de::Error::missing_field("pending"))?,
                    generation: generation
                        .ok_or_else(|| serde::de::Error::missing_field("generation"))?,
                })
            }
        }

        de.deserialize_struct(
            "CommitGraph",
            &[
                "by_sha",
                "by_tag",
                "refs",
                "buffered",
                "pending",
                "generation",
            ],
            GraphVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{CommitGraph, CommitNode};
    use crate::errors::HistoryErrorCode;
    use crate::types::{AppliedAtMs, CommitSha, ManifestGeneration};

    fn sha(byte: u8) -> CommitSha {
        CommitSha::from_bytes([byte; 20])
    }

    fn mg(v: u64) -> ManifestGeneration {
        match ManifestGeneration::new(v) {
            Ok(g) => g,
            Err(e) => {
                assert!(false, "test helper mg({v}) failed: {e}");
                ManifestGeneration::from_raw(v)
            }
        }
    }

    /// Strict-mode insert helper that fails the test on a typed error.
    fn upsert_strict(g: &mut CommitGraph, node: CommitNode) -> Option<CommitNode> {
        match g.upsert_commit(node) {
            Ok(prior) => prior,
            Err(e) => {
                assert!(false, "upsert_strict: {e}");
                None
            }
        }
    }

    #[test]
    fn node_is_merge_classification() {
        let n0 = CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0));
        let n1 = CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1));
        let n2 = CommitNode::new(sha(2), vec![sha(0), sha(1)], AppliedAtMs::new(2));
        assert!(!n0.is_merge());
        assert!(!n1.is_merge());
        assert!(n2.is_merge());
    }

    #[test]
    fn upsert_then_lookup() {
        let mut g = CommitGraph::new();
        let n = CommitNode::new(sha(7), Vec::new(), AppliedAtMs::new(11));
        let prior = upsert_strict(&mut g, n.clone());
        assert!(prior.is_none());
        assert_eq!(g.commit_count(), 1);
        assert_eq!(g.node(&sha(7)), Some(&n));
        assert_eq!(g.node(&sha(8)), None);
    }

    #[test]
    fn upsert_idempotent_same_node() {
        let mut g = CommitGraph::new();
        let n = CommitNode::new(sha(1), Vec::new(), AppliedAtMs::new(42));
        let first = upsert_strict(&mut g, n.clone());
        assert!(first.is_none());
        let second = upsert_strict(&mut g, n.clone());
        assert_eq!(second, Some(n.clone()));
        assert_eq!(g.commit_count(), 1);
        assert_eq!(g.node(&sha(1)), Some(&n));
    }

    #[test]
    fn upsert_replaces_existing_overwriting_parents() {
        let mut g = CommitGraph::new();
        let root = CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0));
        let _r0 = upsert_strict(&mut g, root);
        let other_root = CommitNode::new(sha(1), Vec::new(), AppliedAtMs::new(1));
        let _r1 = upsert_strict(&mut g, other_root);
        // first child has parents=[sha(0)]
        let child_v1 = CommitNode::new(sha(2), vec![sha(0)], AppliedAtMs::new(2));
        let _r2 = upsert_strict(&mut g, child_v1.clone());
        // overwrite child with parents=[sha(1)]
        let child_v2 = CommitNode::new(sha(2), vec![sha(1)], AppliedAtMs::new(20));
        let prior = upsert_strict(&mut g, child_v2.clone());
        assert_eq!(prior, Some(child_v1));
        assert_eq!(g.node(&sha(2)), Some(&child_v2));
        assert_eq!(g.commit_count(), 3);
    }

    #[test]
    fn upsert_strict_rejects_unknown_parent() {
        let mut g = CommitGraph::new();
        let orphan = CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1));
        match g.upsert_commit(orphan) {
            Ok(_) => assert!(false, "strict mode must reject unknown parent"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryCommitParentUnknown),
        }
        assert_eq!(g.commit_count(), 0);
    }

    #[test]
    fn buffered_child_before_parent_resolves_when_parent_arrives() {
        let mut g = CommitGraph::new().with_buffering();
        let child = CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1));
        match g.upsert_commit(child.clone()) {
            Ok(prior) => assert!(prior.is_none()),
            Err(e) => {
                assert!(false, "buffered mode must accept child-before-parent: {e}");
                return;
            }
        }
        // child sits in the graph; pending queue records the wait.
        assert_eq!(g.commit_count(), 1);
        assert!(g.pending().contains_key(&sha(0)));
        let Some(waiting) = g.pending().get(&sha(0)) else {
            assert!(false, "pending queue must hold sha(0) -> {{sha(1)}}");
            return;
        };
        assert!(waiting.contains(&sha(1)));

        // parent arrives -> resolution flips, pending queue empties.
        let parent = CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0));
        let _p = upsert_strict(&mut g, parent);
        assert!(!g.pending().contains_key(&sha(0)));
        assert_eq!(g.commit_count(), 2);
        assert_eq!(g.node(&sha(0)).map(CommitNode::sha), Some(&sha(0)));
        assert_eq!(g.node(&sha(1)), Some(&child));
    }

    #[test]
    fn non_buffered_child_before_parent_fails_closed() {
        let mut g = CommitGraph::new();
        assert!(!g.is_buffered());
        let child = CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1));
        match g.upsert_commit(child) {
            Ok(_) => assert!(false, "non-buffered must fail closed"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryCommitParentUnknown),
        }
    }

    #[test]
    fn remove_leaf_commit_succeeds() {
        let mut g = CommitGraph::new();
        let n0 = CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0));
        let n1 = CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1));
        let _r0 = upsert_strict(&mut g, n0);
        let _r1 = upsert_strict(&mut g, n1.clone());
        match g.remove_commit(&sha(1)) {
            Ok(Some(prior)) => assert_eq!(prior, n1),
            Ok(None) => assert!(false, "must return prior"),
            Err(e) => assert!(false, "{e}"),
        }
        assert_eq!(g.commit_count(), 1);
        assert!(g.node(&sha(1)).is_none());
    }

    #[test]
    fn remove_non_leaf_returns_typed_error() {
        let mut g = CommitGraph::new();
        let n0 = CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0));
        let n1 = CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1));
        let _r0 = upsert_strict(&mut g, n0);
        let _r1 = upsert_strict(&mut g, n1);
        match g.remove_commit(&sha(0)) {
            Ok(_) => assert!(false, "must refuse removal of commit with children"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryCommitHasChildren),
        }
        // Graph unchanged.
        assert_eq!(g.commit_count(), 2);
        assert!(g.node(&sha(0)).is_some());
    }

    #[test]
    fn remove_nonexistent_returns_none_no_error() {
        let mut g = CommitGraph::new();
        match g.remove_commit(&sha(99)) {
            Ok(None) => {}
            Ok(Some(_)) => assert!(false, "must return None"),
            Err(e) => assert!(false, "must not error: {e}"),
        }
    }

    #[test]
    fn from_prior_preserves_full_state() {
        let mut prior = CommitGraph::new();
        let _p0 = upsert_strict(
            &mut prior,
            CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0)),
        );
        let _p1 = upsert_strict(
            &mut prior,
            CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1)),
        );
        let _t: Option<CommitSha> = prior.add_tag("v1", sha(1));
        let _r: Option<CommitSha> = prior.add_ref("main", sha(1));

        let next = CommitGraph::from_prior(&prior, mg(7));
        assert_eq!(next.commit_count(), 2);
        assert_eq!(next.node(&sha(0)).map(CommitNode::sha), Some(&sha(0)));
        assert_eq!(next.node(&sha(1)).map(CommitNode::sha), Some(&sha(1)));
        assert_eq!(next.tags().get("v1"), Some(&sha(1)));
        assert_eq!(next.refs().get("main"), Some(&sha(1)));
        assert_eq!(next.generation(), Some(mg(7)));
    }

    #[test]
    fn from_prior_with_upserts_replaces_only_target() {
        let mut prior = CommitGraph::new();
        let _p0 = upsert_strict(
            &mut prior,
            CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0)),
        );
        let _p1 = upsert_strict(
            &mut prior,
            CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1)),
        );

        let mut next = CommitGraph::from_prior(&prior, mg(2));
        // overwrite sha(1) with a new applied_at_ms; sha(0) should be untouched.
        let updated = CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(99));
        let returned = upsert_strict(&mut next, updated.clone());
        match returned {
            Some(prior_node) => assert_eq!(prior_node.applied_at_ms(), AppliedAtMs::new(1)),
            None => assert!(false, "must return prior"),
        }
        assert_eq!(next.node(&sha(1)), Some(&updated));
        // sha(0) preserved verbatim
        let Some(n0) = next.node(&sha(0)) else {
            assert!(false, "sha(0) lost");
            return;
        };
        assert_eq!(n0.applied_at_ms(), AppliedAtMs::new(0));
    }

    #[test]
    fn resolve_ref_prefers_refs_over_tags() {
        let mut g = CommitGraph::new();
        let _prev_ref: Option<CommitSha> = g.add_ref("main", sha(1));
        let _prev_tag: Option<CommitSha> = g.add_tag("main", sha(2));
        match g.resolve_ref("main") {
            Ok(s) => assert_eq!(*s, sha(1)),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn resolve_ref_falls_back_to_tag() {
        let mut g = CommitGraph::new();
        let _prev_tag: Option<CommitSha> = g.add_tag("v1.0", sha(3));
        match g.resolve_ref("v1.0") {
            Ok(s) => assert_eq!(*s, sha(3)),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn resolve_ref_unknown_fails_closed() {
        let g = CommitGraph::new();
        match g.resolve_ref("nope") {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn cbor_roundtrip_graph() {
        let mut g = CommitGraph::new();
        let _r0 = upsert_strict(
            &mut g,
            CommitNode::new(sha(1), Vec::new(), AppliedAtMs::new(100)),
        );
        let _r1 = upsert_strict(
            &mut g,
            CommitNode::new(sha(2), vec![sha(1)], AppliedAtMs::new(200)),
        );
        let _r2 = upsert_strict(
            &mut g,
            CommitNode::new(sha(3), vec![sha(1), sha(2)], AppliedAtMs::new(300)),
        );
        let _prev_tag: Option<CommitSha> = g.add_tag("v1", sha(2));
        let _prev_ref: Option<CommitSha> = g.add_ref("main", sha(3));

        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&g, &mut buf) {
            assert!(false, "ser: {e}");
        }
        let back: CommitGraph = match ciborium::de::from_reader(buf.as_slice()) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "de: {e}");
                return;
            }
        };
        assert_eq!(back, g);
    }

    #[test]
    fn cbor_roundtrip_is_byte_identical() {
        let mut g = CommitGraph::new();
        for i in 0u8..8 {
            let parents = if i == 0 { Vec::new() } else { vec![sha(i - 1)] };
            let _r = upsert_strict(
                &mut g,
                CommitNode::new(sha(i), parents, AppliedAtMs::new(u64::from(i))),
            );
        }
        let mut a: Vec<u8> = Vec::new();
        let mut b: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&g, &mut a) {
            assert!(false, "{e}");
        }
        if let Err(e) = ciborium::ser::into_writer(&g, &mut b) {
            assert!(false, "{e}");
        }
        assert_eq!(a, b);
    }

    #[test]
    fn cbor_roundtrip_with_buffered_and_generation() {
        let mut g = CommitGraph::new().with_buffering();
        let _r = match g.upsert_commit(CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1))) {
            Ok(p) => p,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let next = CommitGraph::from_prior(&g, mg(5));
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&next, &mut buf) {
            assert!(false, "{e}");
        }
        match ciborium::de::from_reader::<CommitGraph, _>(buf.as_slice()) {
            Ok(back) => assert_eq!(back, next),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
