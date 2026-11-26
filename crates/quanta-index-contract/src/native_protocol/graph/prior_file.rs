//! Exact predecessor data for the file-owned RepoMap producer.

use std::collections::BTreeSet;

use crate::{
    RepoMapNode, RepoMapNodeRef, RepoRelativePath,
    bounded_cluster_members::BoundedVecV1,
    index::{IndexInstanceId, ObjectRef, OwnerKey, ViewRef},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};

use super::{
    GraphEdge, MAX_NATIVE_GRAPH_EDGES, MAX_NATIVE_GRAPH_ITEMS, NativeGraphResult,
    NativeRepoMapFileFactsV1,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeRepoMapPriorNodeDegreeV1 {
    pub incoming: u32,
    pub outgoing: u32,
}
crate::native_serde_struct!(NativeRepoMapPriorNodeDegreeV1 {
    incoming: u32,
    outgoing: u32
});

/// The file's prior contribution and complete distinct degrees in facts-node order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRepoMapPriorFileV1 {
    pub contribution: ObjectRef,
    pub source_owner: OwnerKey,
    pub source: ObjectRef,
    pub output: ObjectRef,
    pub facts: NativeRepoMapFileFactsV1,
    pub edges: Vec<GraphEdge>,
    pub degrees: Vec<NativeRepoMapPriorNodeDegreeV1>,
}

/// Checked old-file endpoint closure for an exact predecessor Graph view.
/// This is an in-process producer input; it does not change the V1 wire.
pub struct NativeRepoMapPriorAffectedV1 {
    pub endpoints: BTreeSet<RepoMapNodeRef>,
    pub incoming_edges: Vec<GraphEdge>,
    pending_incoming: Vec<(RepoMapNodeRef, u32)>,
    next_incoming: usize,
    view: ViewRef,
    instance: IndexInstanceId,
    seen_incoming: BTreeSet<(String, RepoMapNodeRef, RepoMapNodeRef)>,
}

impl NativeRepoMapPriorAffectedV1 {
    /// Q's old outgoing output and reverse adjacency jointly cover cross-file
    /// incidents. Replies are consumed one at a time under the caller's read grant.
    pub fn begin_v1(
        prior: &NativeRepoMapPriorFileV1,
        view: ViewRef,
        instance: IndexInstanceId,
    ) -> Result<Self, &'static str> {
        if prior.facts.nodes.is_empty()
            || prior.facts.nodes.len() != prior.degrees.len()
            || prior.facts.nodes.len().saturating_add(prior.edges.len()) > MAX_NATIVE_GRAPH_ITEMS
        {
            return Err("prior RepoMap affected file geometry differs");
        }
        let mut endpoints = BTreeSet::new();
        for node in &prior.facts.nodes {
            if node_owner_path_v1(node) != &prior.facts.owner_path
                || !endpoints.insert(node_identity_v1(node))
            {
                return Err("prior RepoMap affected node ownership differs");
            }
        }
        let owned = endpoints.clone();
        let mut old_edges = BTreeSet::new();
        for edge in &prior.edges {
            if !owned.contains(&edge.source)
                || !old_edges.insert((edge.kind.as_code_str(), &edge.source, &edge.target))
            {
                return Err("prior RepoMap affected outgoing edge differs");
            }
            let _ = endpoints.insert(edge.target.clone());
        }
        let mut pending_incoming = Vec::new();
        for (node, degree) in prior.facts.nodes.iter().zip(&prior.degrees) {
            if degree.incoming > MAX_NATIVE_GRAPH_EDGES {
                return Err("prior RepoMap incoming degree exceeds exact query bound");
            }
            if degree.incoming > 0 {
                pending_incoming.push((node_identity_v1(node), degree.incoming));
            }
        }
        if endpoints.len() > MAX_NATIVE_GRAPH_ITEMS {
            return Err("prior RepoMap affected closure exceeds exact bounds");
        }
        Ok(Self {
            endpoints,
            incoming_edges: Vec::new(),
            pending_incoming,
            next_incoming: 0,
            view,
            instance,
            seen_incoming: BTreeSet::new(),
        })
    }

    pub fn next_incoming_v1(&self) -> Option<&RepoMapNodeRef> {
        self.pending_incoming
            .get(self.next_incoming)
            .map(|(node, _)| node)
    }

    pub fn accept_incoming_v1(&mut self, reply: &NativeGraphResult) -> Result<(), &'static str> {
        let (anchor, degree) = self
            .pending_incoming
            .get(self.next_incoming)
            .ok_or("prior RepoMap incoming reply is unexpected")?;
        if reply.view != self.view
            || reply.instance != self.instance
            || !reply.exhaustive
            || reply
                .source
                .as_ref()
                .is_none_or(|source| &source.identity != anchor)
            || reply.source_supporters == 0
            || reply.edges.len() != *degree as usize
        {
            return Err("prior RepoMap incoming reply differs from exact Graph");
        }
        for row in &reply.edges {
            if self.incoming_edges.len() >= MAX_NATIVE_GRAPH_ITEMS
                || row.supporters == 0
                || &row.edge.target != anchor
                || !self.seen_incoming.insert((
                    row.edge.kind.as_code_str().to_owned(),
                    row.edge.source.clone(),
                    row.edge.target.clone(),
                ))
            {
                return Err("prior RepoMap incoming adjacency differs");
            }
            let _ = self.endpoints.insert(row.edge.source.clone());
            self.incoming_edges.push(row.edge.clone());
        }
        if self.endpoints.len() > MAX_NATIVE_GRAPH_ITEMS {
            return Err("prior RepoMap affected closure exceeds exact bounds");
        }
        self.next_incoming += 1;
        Ok(())
    }

    pub fn finish_v1(self) -> Result<Self, &'static str> {
        if self.next_incoming != self.pending_incoming.len() {
            return Err("prior RepoMap incoming reply is absent");
        }
        Ok(self)
    }
}

fn node_identity_v1(node: &RepoMapNode) -> RepoMapNodeRef {
    match node {
        RepoMapNode::File(file) => RepoMapNodeRef::File(file.file_id.clone()),
        RepoMapNode::Module(module) => RepoMapNodeRef::Module(module.module_id.clone()),
        RepoMapNode::Symbol(symbol) => RepoMapNodeRef::Symbol(symbol.symbol_id.clone()),
        RepoMapNode::Chunk(chunk) => RepoMapNodeRef::Chunk(chunk.chunk_id.clone()),
    }
}

fn node_owner_path_v1(node: &RepoMapNode) -> &crate::RepoRelativePath {
    match node {
        RepoMapNode::File(file) => &file.repo_relative_path,
        RepoMapNode::Module(module) => &module.repo_relative_path,
        RepoMapNode::Symbol(symbol) => &symbol.owner_path,
        RepoMapNode::Chunk(chunk) => &chunk.owner_path,
    }
}

type PriorEdges = BoundedVecV1<GraphEdge, MAX_NATIVE_GRAPH_ITEMS>;
type PriorDegrees = BoundedVecV1<NativeRepoMapPriorNodeDegreeV1, MAX_NATIVE_GRAPH_ITEMS>;

struct PriorFileWire {
    contribution: ObjectRef,
    source_owner: OwnerKey,
    source: ObjectRef,
    output: ObjectRef,
    facts: NativeRepoMapFileFactsV1,
    edges: PriorEdges,
    degrees: PriorDegrees,
}

crate::native_serde_struct!(PriorFileWire {
    contribution: ObjectRef,
    source_owner: OwnerKey,
    source: ObjectRef,
    output: ObjectRef,
    facts: NativeRepoMapFileFactsV1,
    edges: PriorEdges,
    degrees: PriorDegrees
});

impl Serialize for NativeRepoMapPriorFileV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("NativeRepoMapPriorFileV1", 7)?;
        fields.serialize_field("contribution", &self.contribution)?;
        fields.serialize_field("source_owner", &self.source_owner)?;
        fields.serialize_field("source", &self.source)?;
        fields.serialize_field("output", &self.output)?;
        fields.serialize_field("facts", &self.facts)?;
        fields.serialize_field("edges", &self.edges)?;
        fields.serialize_field("degrees", &self.degrees)?;
        fields.end()
    }
}

impl<'de> Deserialize<'de> for NativeRepoMapPriorFileV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = PriorFileWire::deserialize(deserializer)?;
        let edges = wire.edges.into_inner();
        let degrees = wire.degrees.into_inner();
        let nodes = wire.facts.nodes.len();
        if nodes == 0
            || nodes.saturating_add(edges.len()) > MAX_NATIVE_GRAPH_ITEMS
            || degrees.len() != nodes
        {
            return Err(de::Error::custom(
                "prior RepoMap file has inconsistent or overbound graph data",
            ));
        }
        Ok(Self {
            contribution: wire.contribution,
            source_owner: wire.source_owner,
            source: wire.source,
            output: wire.output,
            facts: wire.facts,
            edges,
            degrees,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRepoMapPriorFileResultV1 {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub owner: OwnerKey,
    pub file: Option<NativeRepoMapPriorFileV1>,
}
crate::native_serde_struct!(NativeRepoMapPriorFileResultV1 {
    view: ViewRef,
    instance: IndexInstanceId,
    owner: OwnerKey,
    file: Option<NativeRepoMapPriorFileV1>
});

/// Exact predecessor endpoint lookup is bounded independently from a file's
/// graph size. A denser endpoint refuses instead of returning a partial set.
pub const MAX_NATIVE_REPOMAP_ENDPOINT_OWNERS_V2: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRepoMapEndpointOwnerV2 {
    pub owner: OwnerKey,
    pub owner_path: RepoRelativePath,
    pub source_owner: OwnerKey,
    pub source: ObjectRef,
    pub contribution: ObjectRef,
    pub output: ObjectRef,
    pub rank_leaf: Option<ObjectRef>,
}
crate::native_serde_struct!(NativeRepoMapEndpointOwnerV2 {
    owner: OwnerKey,
    owner_path: RepoRelativePath,
    source_owner: OwnerKey,
    source: ObjectRef,
    contribution: ObjectRef,
    output: ObjectRef,
    rank_leaf: Option<ObjectRef>
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRepoMapEndpointOwnersResultV2 {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub endpoint: RepoMapNodeRef,
    pub owners: Vec<NativeRepoMapEndpointOwnerV2>,
}

struct EndpointOwnersWireV2 {
    view: ViewRef,
    instance: IndexInstanceId,
    endpoint: RepoMapNodeRef,
    owners: EndpointOwnerRowsV2,
}
type EndpointOwnerRowsV2 =
    BoundedVecV1<NativeRepoMapEndpointOwnerV2, MAX_NATIVE_REPOMAP_ENDPOINT_OWNERS_V2>;
crate::native_serde_struct!(EndpointOwnersWireV2 {
    view: ViewRef,
    instance: IndexInstanceId,
    endpoint: RepoMapNodeRef,
    owners: EndpointOwnerRowsV2
});

impl Serialize for NativeRepoMapEndpointOwnersResultV2 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.owners.len() > MAX_NATIVE_REPOMAP_ENDPOINT_OWNERS_V2 {
            return Err(serde::ser::Error::custom(
                "RepoMap endpoint owners exceed bound",
            ));
        }
        let mut fields = serializer.serialize_struct("NativeRepoMapEndpointOwnersResultV2", 4)?;
        fields.serialize_field("view", &self.view)?;
        fields.serialize_field("instance", &self.instance)?;
        fields.serialize_field("endpoint", &self.endpoint)?;
        fields.serialize_field("owners", &self.owners)?;
        fields.end()
    }
}

impl<'de> Deserialize<'de> for NativeRepoMapEndpointOwnersResultV2 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = EndpointOwnersWireV2::deserialize(deserializer)?;
        let owners = wire.owners.into_inner();
        if owners.windows(2).any(|pair| pair[0].owner >= pair[1].owner) {
            return Err(de::Error::custom(
                "RepoMap endpoint owners are not ordered and unique",
            ));
        }
        Ok(Self {
            view: wire.view,
            instance: wire.instance,
            endpoint: wire.endpoint,
            owners,
        })
    }
}

struct BorrowedResult<'a> {
    view: ViewRef,
    instance: IndexInstanceId,
    owner: OwnerKey,
    file: Option<&'a NativeRepoMapPriorFileV1>,
}

impl Serialize for BorrowedResult<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("NativeRepoMapPriorFileResultV1", 4)?;
        fields.serialize_field("view", &self.view)?;
        fields.serialize_field("instance", &self.instance)?;
        fields.serialize_field("owner", &self.owner)?;
        fields.serialize_field("file", &self.file)?;
        fields.end()
    }
}

impl crate::native_protocol::NativeIndexResponse {
    /// Keep the exact file data under its query allocation grant through serialization.
    pub fn serialize_repomap_prior_file_ref_v1<S: Serializer>(
        view: ViewRef,
        instance: IndexInstanceId,
        owner: OwnerKey,
        file: Option<&NativeRepoMapPriorFileV1>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant(
            "NativeIndexResponse",
            71,
            "RepoMapPriorFileV1",
            &BorrowedResult {
                view,
                instance,
                owner,
                file,
            },
        )
    }
}

#[cfg(test)]
mod affected_tests {
    use super::super::{GraphNode, NativeGraphRow};
    use super::*;
    use crate::{
        ChunkId, RepoMapChunkExactness, RepoMapChunkNode, RepoMapEdgeKind, RepoRelativePath,
        SymbolId,
        index::{
            CollectionId, Digest, ProducerId, SlotId, StableOwnerId, TenantId, WorkspaceKey,
            WorktreeId,
        },
        lex::LanguageCode,
    };

    #[test]
    fn prior_affected_closure_requires_exact_cross_file_incoming_owns_chunk() {
        let view = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 2,
            digest: Digest([5; 32]),
        };
        let instance = IndexInstanceId([6; 16]);
        let chunk = RepoMapNodeRef::Chunk(ChunkId::new("src/b.rs::chunk"));
        let remote = RepoMapNodeRef::Symbol(SymbolId::new("src/a.rs::A"));
        let owner = OwnerKey {
            producer: ProducerId([7; 16]),
            stable_id: StableOwnerId([8; 16]),
        };
        let prior = NativeRepoMapPriorFileV1 {
            contribution: ObjectRef(Digest([9; 32])),
            source_owner: owner,
            source: ObjectRef(Digest([10; 32])),
            output: ObjectRef(Digest([11; 32])),
            facts: NativeRepoMapFileFactsV1 {
                owner_path: RepoRelativePath::new("src/b.rs"),
                nodes: vec![RepoMapNode::Chunk(RepoMapChunkNode {
                    chunk_id: ChunkId::new("src/b.rs::chunk"),
                    owner_path: RepoRelativePath::new("src/b.rs"),
                    language: LanguageCode::new("rust").expect("canonical language"),
                    start_byte: 0,
                    end_byte: 4,
                    start_line: 1,
                    end_line: 1,
                    token_count: 1,
                    preview_text: "body".into(),
                    exactness: RepoMapChunkExactness::Exact,
                })],
            },
            edges: vec![],
            degrees: vec![NativeRepoMapPriorNodeDegreeV1 {
                incoming: 1,
                outgoing: 0,
            }],
        };
        let incoming = NativeGraphResult {
            view,
            instance,
            source: Some(GraphNode {
                identity: chunk.clone(),
                payload_digest: Digest([12; 32]),
            }),
            source_supporters: 1,
            edges: vec![NativeGraphRow {
                edge: GraphEdge {
                    kind: RepoMapEdgeKind::OwnsChunk,
                    source: remote.clone(),
                    target: chunk.clone(),
                },
                supporters: 1,
            }],
            exhaustive: true,
        };
        let mut closure =
            NativeRepoMapPriorAffectedV1::begin_v1(&prior, view, instance).expect("old file");
        assert_eq!(closure.next_incoming_v1(), Some(&chunk));
        closure
            .accept_incoming_v1(&incoming)
            .expect("exact incoming edge");
        let closure = closure.finish_v1().expect("complete closure");
        assert!(closure.endpoints.contains(&remote));
        assert_eq!(closure.incoming_edges, vec![incoming.edges[0].edge.clone()]);

        for changed in [
            NativeGraphResult {
                view: ViewRef {
                    sequence: 3,
                    ..view
                },
                ..incoming.clone()
            },
            NativeGraphResult {
                exhaustive: false,
                ..incoming.clone()
            },
            NativeGraphResult {
                edges: vec![],
                ..incoming.clone()
            },
            NativeGraphResult {
                edges: vec![NativeGraphRow {
                    edge: GraphEdge {
                        target: remote.clone(),
                        ..incoming.edges[0].edge.clone()
                    },
                    supporters: 1,
                }],
                ..incoming.clone()
            },
        ] {
            let mut closure =
                NativeRepoMapPriorAffectedV1::begin_v1(&prior, view, instance).expect("old file");
            assert!(closure.accept_incoming_v1(&changed).is_err());
        }
        assert!(
            NativeRepoMapPriorAffectedV1::begin_v1(&prior, view, instance)
                .expect("old file")
                .finish_v1()
                .is_err()
        );
        let mut over_bound = prior;
        over_bound.degrees[0].incoming = MAX_NATIVE_GRAPH_EDGES + 1;
        assert!(NativeRepoMapPriorAffectedV1::begin_v1(&over_bound, view, instance).is_err());
    }

    #[test]
    fn endpoint_owner_v2_wire_is_bounded_and_ordered() {
        let view = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 1,
            digest: Digest([5; 32]),
        };
        let row = NativeRepoMapEndpointOwnerV2 {
            owner: OwnerKey {
                producer: ProducerId([6; 16]),
                stable_id: StableOwnerId([7; 16]),
            },
            owner_path: RepoRelativePath::new("src/a.rs"),
            source_owner: OwnerKey {
                producer: ProducerId([8; 16]),
                stable_id: StableOwnerId([7; 16]),
            },
            source: ObjectRef(Digest([9; 32])),
            contribution: ObjectRef(Digest([10; 32])),
            output: ObjectRef(Digest([11; 32])),
            rank_leaf: Some(ObjectRef(Digest([12; 32]))),
        };
        let result = NativeRepoMapEndpointOwnersResultV2 {
            view,
            instance: IndexInstanceId([13; 16]),
            endpoint: RepoMapNodeRef::Symbol(SymbolId::new("src/a.rs::A")),
            owners: vec![row.clone()],
        };
        let encoded = serde_json::to_vec(&result).expect("bounded endpoint wire");
        assert_eq!(
            serde_json::from_slice::<NativeRepoMapEndpointOwnersResultV2>(&encoded)
                .expect("exact endpoint wire"),
            result
        );
        let mut duplicate = result.clone();
        duplicate.owners.push(row.clone());
        assert!(
            serde_json::from_slice::<NativeRepoMapEndpointOwnersResultV2>(
                &serde_json::to_vec(&duplicate).expect("duplicate fixture")
            )
            .is_err()
        );
        let mut over_bound = result;
        over_bound.owners = vec![row; MAX_NATIVE_REPOMAP_ENDPOINT_OWNERS_V2 + 1];
        assert!(serde_json::to_vec(&over_bound).is_err());
    }
}
