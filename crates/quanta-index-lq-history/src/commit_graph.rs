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
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;
use std::collections::BTreeMap;

use crate::errors::{HistoryError, HistoryErrorCode};
use crate::types::{AppliedAtMs, CommitSha};

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
}

impl CommitGraph {
    /// Empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_sha: BTreeMap::new(),
            by_tag: BTreeMap::new(),
            refs: BTreeMap::new(),
        }
    }

    /// Insert a commit. Overwrites any prior node at the same sha.
    pub fn add_commit(&mut self, node: CommitNode) {
        let sha = *node.sha();
        drop(self.by_sha.insert(sha, node));
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
        let mut st = ser.serialize_struct("CommitGraph", 3)?;
        st.serialize_field("by_sha", &self.by_sha)?;
        st.serialize_field("by_tag", &self.by_tag)?;
        st.serialize_field("refs", &self.refs)?;
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
                            other => Err(E::unknown_field(other, &["by_sha", "by_tag", "refs"])),
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
                    }
                }
                Ok(CommitGraph {
                    by_sha: by_sha.ok_or_else(|| serde::de::Error::missing_field("by_sha"))?,
                    by_tag: by_tag.ok_or_else(|| serde::de::Error::missing_field("by_tag"))?,
                    refs: refs.ok_or_else(|| serde::de::Error::missing_field("refs"))?,
                })
            }
        }

        de.deserialize_struct("CommitGraph", &["by_sha", "by_tag", "refs"], GraphVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::{CommitGraph, CommitNode};
    use crate::errors::HistoryErrorCode;
    use crate::types::{AppliedAtMs, CommitSha};

    fn sha(byte: u8) -> CommitSha {
        CommitSha::from_bytes([byte; 20])
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
    fn add_commit_then_lookup() {
        let mut g = CommitGraph::new();
        let n = CommitNode::new(sha(7), Vec::new(), AppliedAtMs::new(11));
        g.add_commit(n.clone());
        assert_eq!(g.commit_count(), 1);
        assert_eq!(g.node(&sha(7)), Some(&n));
        assert_eq!(g.node(&sha(8)), None);
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
        g.add_commit(CommitNode::new(sha(1), Vec::new(), AppliedAtMs::new(100)));
        g.add_commit(CommitNode::new(sha(2), vec![sha(1)], AppliedAtMs::new(200)));
        g.add_commit(CommitNode::new(
            sha(3),
            vec![sha(1), sha(2)],
            AppliedAtMs::new(300),
        ));
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
            g.add_commit(CommitNode::new(
                sha(i),
                parents,
                AppliedAtMs::new(u64::from(i)),
            ));
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
}
