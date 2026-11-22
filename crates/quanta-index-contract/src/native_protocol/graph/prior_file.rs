//! Exact predecessor data for the file-owned RepoMap producer.

use crate::{
    bounded_cluster_members::BoundedVecV1,
    index::{IndexInstanceId, ObjectRef, OwnerKey, ViewRef},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};

use super::{GraphEdge, MAX_NATIVE_GRAPH_ITEMS, NativeRepoMapFileFactsV1};

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
