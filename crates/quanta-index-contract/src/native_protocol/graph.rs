//! Canonical certified graph owner output and exact adjacency wire.

use crate::{
    RepoMapEdgeKind, RepoMapNodeRef,
    index::{Digest, IndexInstanceId, ViewRef},
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, SeqAccess, Visitor},
    ser::SerializeStruct,
};
use std::fmt;

mod ranking;
pub use ranking::*;

pub const MAX_NATIVE_GRAPH_ITEMS: usize = 4096;
pub const MAX_NATIVE_GRAPH_EDGES: u32 = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphNode {
    pub identity: RepoMapNodeRef,
    pub payload_digest: Digest,
}
crate::native_serde_struct!(GraphNode {
    identity: RepoMapNodeRef,
    payload_digest: Digest
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphEdge {
    pub kind: RepoMapEdgeKind,
    pub source: RepoMapNodeRef,
    pub target: RepoMapNodeRef,
}
crate::native_serde_struct!(GraphEdge {
    kind: RepoMapEdgeKind,
    source: RepoMapNodeRef,
    target: RepoMapNodeRef
});

/// Producer-certified immutable payload of `ProjectionOwnerContribution.output`.
/// Consumers never reconstruct this from source bytes or a second IR.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphOwnerOutput {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub projection: Vec<GraphProjectionRow>,
}

impl Serialize for GraphOwnerOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("GraphOwnerOutput", 3)?;
        fields.serialize_field("nodes", &self.nodes)?;
        fields.serialize_field("edges", &self.edges)?;
        fields.serialize_field("projection", &self.projection)?;
        fields.end()
    }
}

struct LimitedNodes(Vec<GraphNode>);
struct LimitedEdges(Vec<GraphEdge>);
struct LimitedProjection(Vec<GraphProjectionRow>);

macro_rules! limited_graph_vec {
    ($name:ident, $element:ty) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct Entries;
                impl<'de> Visitor<'de> for Entries {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a bounded graph array")
                    }
                    fn visit_seq<A: SeqAccess<'de>>(
                        self,
                        mut seq: A,
                    ) -> Result<Self::Value, A::Error> {
                        let mut items = Vec::new();
                        while let Some(item) = seq.next_element::<$element>()? {
                            if items.len() >= MAX_NATIVE_GRAPH_ITEMS {
                                return Err(de::Error::custom("graph output exceeds item bound"));
                            }
                            items.try_reserve(1).map_err(|_| {
                                de::Error::custom("graph output allocation refused")
                            })?;
                            items.push(item);
                        }
                        Ok($name(items))
                    }
                }
                deserializer.deserialize_seq(Entries)
            }
        }
    };
}
limited_graph_vec!(LimitedNodes, GraphNode);
limited_graph_vec!(LimitedEdges, GraphEdge);
limited_graph_vec!(LimitedProjection, GraphProjectionRow);

impl<'de> Deserialize<'de> for GraphOwnerOutput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Output;
        impl<'de> Visitor<'de> for Output {
            type Value = GraphOwnerOutput;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a certified graph owner output")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut nodes = None;
                let mut edges = None;
                let mut projection = None;
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "nodes" if nodes.is_none() => {
                            nodes = Some(map.next_value::<LimitedNodes>()?.0);
                        }
                        "edges" if edges.is_none() => {
                            edges = Some(map.next_value::<LimitedEdges>()?.0);
                        }
                        "projection" if projection.is_none() => {
                            projection = Some(map.next_value::<LimitedProjection>()?.0);
                        }
                        "projection" => return Err(de::Error::duplicate_field("projection")),
                        "nodes" => return Err(de::Error::duplicate_field("nodes")),
                        "edges" => return Err(de::Error::duplicate_field("edges")),
                        _ => {
                            return Err(de::Error::unknown_field(
                                &field,
                                &["nodes", "edges", "projection"],
                            ));
                        }
                    }
                }
                let nodes: Vec<GraphNode> =
                    nodes.ok_or_else(|| de::Error::missing_field("nodes"))?;
                let edges: Vec<GraphEdge> =
                    edges.ok_or_else(|| de::Error::missing_field("edges"))?;
                if nodes
                    .len()
                    .checked_add(edges.len())
                    .is_none_or(|count| count > MAX_NATIVE_GRAPH_ITEMS)
                {
                    return Err(de::Error::custom("graph output exceeds item bound"));
                }
                let projection =
                    projection.ok_or_else(|| de::Error::missing_field("projection"))?;
                Ok(GraphOwnerOutput {
                    nodes,
                    edges,
                    projection,
                })
            }
        }
        deserializer.deserialize_struct(
            "GraphOwnerOutput",
            &["nodes", "edges", "projection"],
            Output,
        )
    }
}

/// Which endpoint of each returned directed edge is the query anchor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeGraphDirection {
    Outgoing,
    Incoming,
}
quanta_index_contract_base::native_serde_enum!(NativeGraphDirection {
    units { Outgoing = 0, Incoming = 1 } newtypes {} records {}
});

impl NativeGraphDirection {
    #[must_use]
    pub fn endpoint(self, edge: &GraphEdge) -> &RepoMapNodeRef {
        match self {
            Self::Outgoing => &edge.source,
            Self::Incoming => &edge.target,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeGraphQuery {
    /// Query anchor: the source for outgoing edges, target for incoming edges.
    pub source: RepoMapNodeRef,
    pub direction: NativeGraphDirection,
    /// Maximum supporters of the anchor node and each distinct edge.
    pub max_supporters: u32,
    pub max_edges: u32,
}
crate::native_serde_struct!(NativeGraphQuery {
    source: RepoMapNodeRef,
    direction: NativeGraphDirection,
    max_supporters: u32,
    max_edges: u32
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeGraphRow {
    pub edge: GraphEdge,
    pub supporters: u32,
}
crate::native_serde_struct!(NativeGraphRow {
    edge: GraphEdge,
    supporters: u32
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeGraphResult {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    /// Selected query anchor, irrespective of edge direction.
    pub source: Option<GraphNode>,
    pub source_supporters: u32,
    pub edges: Vec<NativeGraphRow>,
    pub exhaustive: bool,
}
crate::native_serde_struct!(NativeGraphResult {
    view: ViewRef,
    instance: IndexInstanceId,
    source: Option<GraphNode>,
    source_supporters: u32,
    edges: Vec<NativeGraphRow>,
    exhaustive: bool
});

impl super::NativeIndexResponse {
    /// Borrow the graph result until the runtime's retained allocation is released.
    pub fn serialize_graph_ref<S: serde::Serializer>(
        result: &NativeGraphResult,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 26, "Graph", result)
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeGraphDirection, NativeGraphQuery};
    use crate::{FileId, RepoMapNodeRef};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn graph_projection_refs_are_required_and_bounded_in_owner_output() -> TestResult {
        let empty = r#"{"nodes":[],"edges":[],"projection":[]}"#;
        let output: super::GraphOwnerOutput = serde_json::from_str(empty)?;
        if !output.projection.is_empty() || serde_json::to_string(&output)? != empty {
            return Err("graph output projection contract changed".into());
        }
        for invalid in [
            r#"{"nodes":[],"edges":[]}"#,
            r#"{"nodes":[],"edges":[],"projection":[],"projection":[]}"#,
            r#"{"nodes":[],"edges":[],"projection":[],"unknown":0}"#,
        ] {
            if serde_json::from_str::<super::GraphOwnerOutput>(invalid).is_ok() {
                return Err("graph output accepted missing or malformed projection refs".into());
            }
        }
        Ok(())
    }

    #[test]
    fn graph_direction_wire_has_fixed_names_and_no_implicit_default() -> TestResult {
        for (text, direction) in [
            (r#""Outgoing""#, NativeGraphDirection::Outgoing),
            (r#""Incoming""#, NativeGraphDirection::Incoming),
        ] {
            let decoded = serde_json::from_str::<NativeGraphDirection>(text)?;
            if decoded != direction || serde_json::to_string(&direction)? != text {
                return Err("graph direction wire changed".into());
            }
        }
        let incoming = r#"{"source":{"File":"src/b.rs"},"direction":"Incoming","max_supporters":2,"max_edges":4}"#;
        let decoded = serde_json::from_str::<NativeGraphQuery>(incoming)?;
        if decoded.source != RepoMapNodeRef::File(FileId::new("src/b.rs"))
            || decoded.direction != NativeGraphDirection::Incoming
            || decoded.max_supporters != 2
            || decoded.max_edges != 4
            || serde_json::to_string(&decoded)? != incoming
        {
            return Err("graph query direction or anchor changed".into());
        }
        for invalid in [
            r#"{"source":{"File":"src/b.rs"},"max_supporters":2,"max_edges":4}"#,
            r#"{"source":{"File":"src/b.rs"},"direction":"Both","max_supporters":2,"max_edges":4}"#,
            r#"{"source":{"File":"src/b.rs"},"direction":"Incoming","direction":"Outgoing","max_supporters":2,"max_edges":4}"#,
        ] {
            if serde_json::from_str::<NativeGraphQuery>(invalid).is_ok() {
                return Err("graph direction was defaulted or malformed input accepted".into());
            }
        }
        Ok(())
    }
}
