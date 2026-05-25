//! `revisions:<range>` enumeration primitive.
//!
//! Supports two-dot (`a..b`) and three-dot (`a...b`) ranges over the commit
//! DAG.
//!
//! - Two-dot: commits reachable from `b` but not from `a` (i.e. set
//!   difference `ancestors(b) \ ancestors(a)`).
//! - Three-dot: symmetric difference
//!   `(ancestors(a) ∪ ancestors(b)) \ (ancestors(a) ∩ ancestors(b))`.
//!
//! Both forms cap the returned set at `max`; over-cap surfaces
//! [`HistoryErrorCode::PlanLimitExceeded`] with
//! [`LimitDimension::RevisionsCount`]. Either endpoint missing surfaces
//! [`HistoryErrorCode::HistoryRefNotFound`].
//!
//! D18 — manual serde for [`RevisionRange`]; no proc-macro derives.

use core::fmt;
use std::collections::BTreeSet;

use crate::commit_graph::CommitGraph;
use crate::errors::{HistoryError, HistoryErrorCode, LimitDimension};
use crate::types::CommitSha;

/// Range form recognised by `revisions:<range>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RevisionRange {
    /// Two-dot `a..b`: reachable from `b`, not from `a`.
    TwoDot {
        /// Excluded endpoint.
        a: CommitSha,
        /// Included endpoint.
        b: CommitSha,
    },
    /// Three-dot `a...b`: symmetric difference of ancestor sets.
    ThreeDot {
        /// Left endpoint.
        a: CommitSha,
        /// Right endpoint.
        b: CommitSha,
    },
}

impl serde::Serialize for RevisionRange {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("RevisionRange", 3)?;
        match *self {
            Self::TwoDot { a, b } => {
                st.serialize_field("kind", "TwoDot")?;
                st.serialize_field("a", &a)?;
                st.serialize_field("b", &b)?;
            }
            Self::ThreeDot { a, b } => {
                st.serialize_field("kind", "ThreeDot")?;
                st.serialize_field("a", &a)?;
                st.serialize_field("b", &b)?;
            }
        }
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for RevisionRange {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Clone, Copy)]
        enum Field {
            Kind,
            A,
            B,
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
                        f.write_str("RevisionRange field name")
                    }
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                        match v {
                            "kind" => Ok(Field::Kind),
                            "a" => Ok(Field::A),
                            "b" => Ok(Field::B),
                            other => Err(E::unknown_field(other, &["kind", "a", "b"])),
                        }
                    }
                }
                de.deserialize_str(V)
            }
        }

        struct RV;
        impl<'d> serde::de::Visitor<'d> for RV {
            type Value = RevisionRange;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("RevisionRange struct")
            }
            fn visit_map<A: serde::de::MapAccess<'d>>(
                self,
                mut map: A,
            ) -> Result<RevisionRange, A::Error> {
                let mut kind: Option<String> = None;
                let mut a: Option<CommitSha> = None;
                let mut b: Option<CommitSha> = None;
                while let Some(k) = map.next_key::<Field>()? {
                    match k {
                        Field::Kind => {
                            if kind.is_some() {
                                return Err(serde::de::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value()?);
                        }
                        Field::A => {
                            if a.is_some() {
                                return Err(serde::de::Error::duplicate_field("a"));
                            }
                            a = Some(map.next_value()?);
                        }
                        Field::B => {
                            if b.is_some() {
                                return Err(serde::de::Error::duplicate_field("b"));
                            }
                            b = Some(map.next_value()?);
                        }
                    }
                }
                let kind = kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?;
                let a = a.ok_or_else(|| serde::de::Error::missing_field("a"))?;
                let b = b.ok_or_else(|| serde::de::Error::missing_field("b"))?;
                match kind.as_str() {
                    "TwoDot" => Ok(RevisionRange::TwoDot { a, b }),
                    "ThreeDot" => Ok(RevisionRange::ThreeDot { a, b }),
                    other => Err(serde::de::Error::unknown_variant(
                        other,
                        &["TwoDot", "ThreeDot"],
                    )),
                }
            }
        }

        de.deserialize_struct("RevisionRange", &["kind", "a", "b"], RV)
    }
}

/// Enumerate commits matched by `range`, capped at `max`.
pub fn enumerate_revisions(
    graph: &CommitGraph,
    range: &RevisionRange,
    max: u32,
) -> Result<Vec<CommitSha>, HistoryError> {
    let result = match *range {
        RevisionRange::TwoDot { a, b } => {
            let anc_a = ancestors(graph, &a)?;
            let anc_b = ancestors(graph, &b)?;
            anc_b.difference(&anc_a).copied().collect::<BTreeSet<_>>()
        }
        RevisionRange::ThreeDot { a, b } => {
            let anc_a = ancestors(graph, &a)?;
            let anc_b = ancestors(graph, &b)?;
            anc_a
                .symmetric_difference(&anc_b)
                .copied()
                .collect::<BTreeSet<_>>()
        }
    };

    let len_u32 = u32::try_from(result.len()).map_err(|_too_large| {
        HistoryError::plan_limit(
            LimitDimension::RevisionsCount,
            format!(
                "revisions: enumerated {} commits, exceeds u32::MAX",
                result.len()
            ),
        )
    })?;
    if len_u32 > max {
        return Err(HistoryError::plan_limit(
            LimitDimension::RevisionsCount,
            format!("revisions: enumerated {len_u32} commits, exceeds cap {max}"),
        ));
    }
    Ok(result.into_iter().collect())
}

/// Collect every ancestor sha reachable from `root` (inclusive).
///
/// Surfaces `HistoryRefNotFound` when `root` is absent.
fn ancestors(graph: &CommitGraph, root: &CommitSha) -> Result<BTreeSet<CommitSha>, HistoryError> {
    if graph.node(root).is_none() {
        return Err(HistoryError::new(
            HistoryErrorCode::HistoryRefNotFound,
            format!("revisions: endpoint commit {root} not found in graph"),
        ));
    }
    let mut seen: BTreeSet<CommitSha> = BTreeSet::new();
    let mut stack: Vec<CommitSha> = Vec::new();
    stack.push(*root);
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        let Some(node) = graph.node(&cur) else {
            return Err(HistoryError::new(
                HistoryErrorCode::HistoryRefNotFound,
                format!("revisions: ancestor commit {cur} referenced but absent"),
            ));
        };
        for p in node.parents() {
            if !seen.contains(p) {
                stack.push(*p);
            }
        }
    }
    Ok(seen)
}

#[cfg(test)]
mod tests {
    use super::{RevisionRange, ancestors, enumerate_revisions};
    use crate::commit_graph::{CommitGraph, CommitNode};
    use crate::errors::{HistoryErrorCode, LimitDimension};
    use crate::types::{AppliedAtMs, CommitSha};

    fn sha(byte: u8) -> CommitSha {
        CommitSha::from_bytes([byte; 20])
    }

    /// Strict-mode upsert helper that fails the test on a typed error.
    fn ups(g: &mut CommitGraph, node: CommitNode) {
        if let Err(e) = g.upsert_commit(node) {
            assert!(false, "upsert_commit: {e}");
        }
    }

    fn diamond() -> CommitGraph {
        // 0 <- 1 <- 3
        //   \- 2 -/
        let mut g = CommitGraph::new();
        ups(
            &mut g,
            CommitNode::new(sha(0), Vec::new(), AppliedAtMs::new(0)),
        );
        ups(
            &mut g,
            CommitNode::new(sha(1), vec![sha(0)], AppliedAtMs::new(1)),
        );
        ups(
            &mut g,
            CommitNode::new(sha(2), vec![sha(0)], AppliedAtMs::new(2)),
        );
        ups(
            &mut g,
            CommitNode::new(sha(3), vec![sha(1), sha(2)], AppliedAtMs::new(3)),
        );
        g
    }

    #[test]
    fn ancestors_includes_root() {
        let g = diamond();
        let s = match ancestors(&g, &sha(3)) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn two_dot_excludes_a_ancestors() {
        let g = diamond();
        // 1..3 -> commits reachable from 3 not from 1.
        // ancestors(3) = {0,1,2,3}; ancestors(1) = {0,1}
        // diff = {2,3}
        let got = match enumerate_revisions(
            &g,
            &RevisionRange::TwoDot {
                a: sha(1),
                b: sha(3),
            },
            100,
        ) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got, vec![sha(2), sha(3)]);
    }

    #[test]
    fn three_dot_symmetric_difference() {
        let g = diamond();
        // 1...2 -> sym diff of {0,1} and {0,2} = {1,2}
        let got = match enumerate_revisions(
            &g,
            &RevisionRange::ThreeDot {
                a: sha(1),
                b: sha(2),
            },
            100,
        ) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(got, vec![sha(1), sha(2)]);
    }

    #[test]
    fn cap_exceeded_surfaces_typed_error() {
        let g = diamond();
        // sym diff size is 2; cap = 1
        match enumerate_revisions(
            &g,
            &RevisionRange::ThreeDot {
                a: sha(1),
                b: sha(2),
            },
            1,
        ) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => {
                assert_eq!(e.code, HistoryErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::RevisionsCount));
            }
        }
    }

    #[test]
    fn unknown_endpoint_fails() {
        let g = diamond();
        match enumerate_revisions(
            &g,
            &RevisionRange::TwoDot {
                a: sha(1),
                b: sha(99),
            },
            100,
        ) {
            Ok(_) => assert!(false, "must fail"),
            Err(e) => assert_eq!(e.code, HistoryErrorCode::HistoryRefNotFound),
        }
    }

    #[test]
    fn revision_range_cbor_roundtrip() {
        let cases = [
            RevisionRange::TwoDot {
                a: sha(1),
                b: sha(2),
            },
            RevisionRange::ThreeDot {
                a: sha(3),
                b: sha(4),
            },
        ];
        for r in cases {
            let mut buf: Vec<u8> = Vec::new();
            if let Err(e) = ciborium::ser::into_writer(&r, &mut buf) {
                assert!(false, "{e}");
            }
            match ciborium::de::from_reader::<RevisionRange, _>(buf.as_slice()) {
                Ok(back) => assert_eq!(back, r),
                Err(e) => assert!(false, "{e}"),
            }
        }
    }
}
