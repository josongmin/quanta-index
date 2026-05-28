//! Durable CBOR encoding for the persisted HNSW serving graph.
//!
//! The graph is built once at seal time and loaded — never rebuilt — at open
//! time, so the open path's cost is bounded by a single sealed generation
//! rather than a full-history replay. Node adjacency indices are stored as
//! `u64`; vectors are stored as little-endian `f32` bytes.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use quanta_index_core::CoreError;

use crate::codec::{self, cbor_serde};
use crate::hnsw::{HnswIndex, PersistedGraph, PersistedNode};
use crate::manifest::FORMAT_VERSION;

struct GraphNodeDoc {
    id: String,
    vector_le_bytes: Vec<u8>,
    adjacency: Vec<Vec<u64>>,
    deleted: bool,
}

cbor_serde!(GraphNodeDoc {
    id: String,
    vector_le_bytes: Vec<u8>,
    adjacency: Vec<Vec<u64>>,
    deleted: bool,
});

struct GraphDoc {
    format_version: u32,
    dim: u64,
    max_level: u64,
    entry: Option<u64>,
    nodes: Vec<GraphNodeDoc>,
}

cbor_serde!(GraphDoc {
    format_version: u32,
    dim: u64,
    max_level: u64,
    entry: Option<u64>,
    nodes: Vec<GraphNodeDoc>,
});

fn index_overflow(err: impl core::fmt::Display) -> CoreError {
    CoreError::Storage(format!("semantic: graph index conversion: {err}"))
}

pub(crate) fn encode_graph(index: &HnswIndex) -> Result<Vec<u8>, CoreError> {
    let persisted = index.to_persisted();
    let mut nodes: Vec<GraphNodeDoc> = Vec::with_capacity(persisted.nodes.len());
    for node in persisted.nodes {
        let mut adjacency: Vec<Vec<u64>> = Vec::with_capacity(node.adjacency.len());
        for layer in node.adjacency {
            let mut row: Vec<u64> = Vec::with_capacity(layer.len());
            for neighbor in layer {
                row.push(u64::try_from(neighbor).map_err(index_overflow)?);
            }
            adjacency.push(row);
        }
        nodes.push(GraphNodeDoc {
            id: node.id,
            vector_le_bytes: codec::f32_slice_to_le_bytes(&node.vector),
            adjacency,
            deleted: node.deleted,
        });
    }
    let entry = match persisted.entry {
        Some(entry) => Some(u64::try_from(entry).map_err(index_overflow)?),
        None => None,
    };
    let doc = GraphDoc {
        format_version: FORMAT_VERSION,
        dim: u64::try_from(persisted.dim).map_err(index_overflow)?,
        max_level: u64::try_from(persisted.max_level).map_err(index_overflow)?,
        entry,
        nodes,
    };
    codec::encode(&doc, "semantic graph")
}

pub(crate) fn decode_graph(bytes: &[u8]) -> Result<HnswIndex, CoreError> {
    let doc: GraphDoc = codec::decode(bytes, "semantic graph")?;
    if doc.format_version != FORMAT_VERSION {
        return Err(CoreError::Storage(format!(
            "semantic: graph format version {} unsupported (expected {FORMAT_VERSION})",
            doc.format_version
        )));
    }
    let dim = usize::try_from(doc.dim).map_err(index_overflow)?;
    let max_level = usize::try_from(doc.max_level).map_err(index_overflow)?;
    let entry = match doc.entry {
        Some(entry) => Some(usize::try_from(entry).map_err(index_overflow)?),
        None => None,
    };
    let mut nodes: Vec<PersistedNode> = Vec::with_capacity(doc.nodes.len());
    for node in doc.nodes {
        let vector = codec::le_bytes_to_f32_vec(&node.vector_le_bytes)?;
        let mut adjacency: Vec<Vec<usize>> = Vec::with_capacity(node.adjacency.len());
        for layer in node.adjacency {
            let mut row: Vec<usize> = Vec::with_capacity(layer.len());
            for neighbor in layer {
                row.push(usize::try_from(neighbor).map_err(index_overflow)?);
            }
            adjacency.push(row);
        }
        nodes.push(PersistedNode {
            id: node.id,
            vector,
            adjacency,
            deleted: node.deleted,
        });
    }
    HnswIndex::from_persisted(PersistedGraph {
        dim,
        max_level,
        entry,
        nodes,
    })
}
