//! Producer-side materialization of a ranked Native graph owner.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, io,
};

use crate::{
    CompiledRepoMapProjectionEntryV1, RepoMapCandidateCommitmentsV1, RepoMapCompileRefusalV1,
    RepoMapNode, RepoMapNodeRef, RepoMapSourceBundle, RepoRelativePath,
    index::{Digest, ObjectRef},
    native_protocol::{
        GraphEdge, GraphNode, GraphOwnerOutput, GraphProjectionRow, MAX_NATIVE_GRAPH_ITEMS,
        MAX_NATIVE_GRAPH_OUTPUT_BYTES, MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN,
        NativeRepoMapFileFactsV1,
    },
};
use sha2::{Digest as _, Sha256};

use crate::repomap::compiler::{RepoMapGraphCompiler, node_identity, producer_digest};

mod input_stamp;

impl NativeRepoMapFileFactsV1 {
    /// Check reopened typed facts against the graph nodes and their producer
    /// digests without trusting the separately stored rank leaves.
    pub fn matches_graph_nodes_v1(
        &self,
        graph_nodes: &[GraphNode],
    ) -> Result<bool, serde_json::Error> {
        if self.nodes.is_empty() || self.nodes.len() != graph_nodes.len() {
            return Ok(false);
        }
        for (node, graph_node) in self.nodes.iter().zip(graph_nodes) {
            let path = node_owner_path(node);
            if path != &self.owner_path || node_identity(node) != graph_node.identity {
                return Ok(false);
            }
            if graph_node_payload_digest_v1(node)? != graph_node.payload_digest {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Debug)]
pub enum NativeRepoMapOwnerError {
    Compile(RepoMapCompileRefusalV1),
    GraphLimit,
    InvalidRankRow,
    Encoding(serde_json::Error),
    RankLeafBinding,
    OwnerPartition,
}

impl fmt::Display for NativeRepoMapOwnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compile(cause) => fmt::Display::fmt(cause, formatter),
            Self::GraphLimit => formatter.write_str("Native RepoMap owner exceeds graph bounds"),
            Self::InvalidRankRow => formatter.write_str("invalid Native RepoMap rank row"),
            Self::Encoding(cause) => write!(formatter, "Native RepoMap encoding: {cause}"),
            Self::RankLeafBinding => {
                formatter.write_str("Native RepoMap rank leaf binding differs")
            }
            Self::OwnerPartition => {
                formatter.write_str("Native RepoMap file owner partition differs")
            }
        }
    }
}

impl std::error::Error for NativeRepoMapOwnerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Compile(cause) => Some(cause),
            Self::Encoding(cause) => Some(cause),
            Self::GraphLimit
            | Self::InvalidRankRow
            | Self::RankLeafBinding
            | Self::OwnerPartition => None,
        }
    }
}

/// Canonical graph facts and compiled rank leaves for one owner contribution.
/// Publish through the Native projection operation and its affected-owner certificate.
pub struct NativeRepoMapOwnerProjection {
    output: GraphOwnerOutput,
    rank_rows: Vec<CompiledRepoMapProjectionEntryV1>,
    commitments: RepoMapCandidateCommitmentsV1,
}

/// One file-owned contribution extracted from a single whole-bundle compile.
/// Cross-file edges belong to their source file and retain the target identity.
pub struct NativeRepoMapFileOwnerProjectionV1 {
    owner_path: RepoRelativePath,
    projection: NativeRepoMapOwnerProjection,
    facts: NativeRepoMapFileFactsV1,
}

impl NativeRepoMapFileOwnerProjectionV1 {
    pub fn owner_path(&self) -> &RepoRelativePath {
        &self.owner_path
    }

    pub fn rank_rows(&self) -> &[CompiledRepoMapProjectionEntryV1] {
        self.projection.rank_rows()
    }

    pub fn file_facts(&self) -> &NativeRepoMapFileFactsV1 {
        &self.facts
    }

    /// Fingerprint the already compiled immutable file contribution. Equal
    /// stamps imply equal facts, adjacency and rank leaves under this policy;
    /// a caller may compare stamps only after the full bundle was compiled.
    pub fn output_stamp_v1(&self) -> Result<Digest, NativeRepoMapOwnerError> {
        let mut hash = Sha256::new();
        hash.update(b"quanta-index/native-repomap-file-output-stamp/v1\0");
        hash.update(crate::native_protocol::native_repomap_rank_policy().0);
        hash.update(self.projection.commitments.schema);
        hash.update(self.projection.commitments.projection_profile);
        stamp_typed_v1(&mut hash, &self.owner_path)?;

        let output = &self.projection.output;
        stamp_count_v1(&mut hash, output.nodes.len());
        for node in &output.nodes {
            stamp_typed_v1(&mut hash, &node.identity)?;
            hash.update(node.payload_digest.0);
        }
        stamp_count_v1(&mut hash, output.edges.len());
        for edge in &output.edges {
            stamp_bytes_v1(&mut hash, edge.kind.as_code_str().as_bytes());
            stamp_typed_v1(&mut hash, &edge.source)?;
            stamp_typed_v1(&mut hash, &edge.target)?;
        }
        stamp_count_v1(&mut hash, self.projection.rank_rows.len());
        for row in &self.projection.rank_rows {
            stamp_typed_v1(&mut hash, &row.subject)?;
            stamp_bytes_v1(&mut hash, row.doc_type.as_code_str().as_bytes());
            stamp_typed_v1(&mut hash, &row.owner_path)?;
            stamp_bytes_v1(&mut hash, row.search_text.as_bytes());
            hash.update(row.final_score_millis.to_le_bytes());
        }
        Ok(Digest(hash.finalize().into()))
    }

    pub fn bind_rank_leaves(
        self,
        leaves: &[GraphProjectionRow],
        facts_ref: ObjectRef,
    ) -> Result<(GraphOwnerOutput, RepoMapCandidateCommitmentsV1), NativeRepoMapOwnerError> {
        let (mut output, commitments) = self.projection.bind_rank_leaves(leaves)?;
        output.facts = Some(facts_ref);
        check_graph_encoding(&output)?;
        Ok((output, commitments))
    }
}

fn stamp_count_v1(hash: &mut Sha256, count: usize) {
    hash.update((count as u64).to_le_bytes());
}

fn stamp_bytes_v1(hash: &mut Sha256, bytes: &[u8]) {
    stamp_count_v1(hash, bytes.len());
    hash.update(bytes);
}

fn stamp_typed_v1<T: serde::Serialize>(
    hash: &mut Sha256,
    value: &T,
) -> Result<(), NativeRepoMapOwnerError> {
    // 0xff cannot appear in a valid UTF-8 JSON value, so it frames adjacent
    // typed identities without allocating or changing their public wire.
    hash.update([0xff]);
    serde_json::to_writer(&mut *hash, value).map_err(NativeRepoMapOwnerError::Encoding)?;
    hash.update([0xff]);
    Ok(())
}

impl NativeRepoMapOwnerProjection {
    /// Stage each row as its canonical JSON leaf in this order.
    pub fn rank_rows(&self) -> &[CompiledRepoMapProjectionEntryV1] {
        &self.rank_rows
    }

    /// Bind the immutable leaf references returned by staging this owner's rows.
    /// Register those references as children when staging the graph output and
    /// retain the returned commitments with the publication authority.
    pub fn bind_rank_leaves(
        mut self,
        leaves: &[GraphProjectionRow],
    ) -> Result<(GraphOwnerOutput, RepoMapCandidateCommitmentsV1), NativeRepoMapOwnerError> {
        if leaves.len() != self.rank_rows.len() {
            return Err(NativeRepoMapOwnerError::RankLeafBinding);
        }
        let mut references = BTreeSet::new();
        for (projection, leaf) in self.output.projection.iter_mut().zip(leaves) {
            if projection.subject != leaf.subject || !references.insert(leaf.row) {
                return Err(NativeRepoMapOwnerError::RankLeafBinding);
            }
            projection.row = leaf.row;
        }
        check_graph_encoding(&self.output)?;
        Ok((self.output, self.commitments))
    }
}

impl RepoMapGraphCompiler {
    /// Preserve global validation, graph degrees and ranks, then partition the
    /// canonical output by the source node's file path. The returned path list
    /// is ordered; the caller binds each path to its admitted Source owner.
    pub fn compile_native_file_owners_v1(
        &self,
        bundle: &RepoMapSourceBundle,
    ) -> Result<Vec<NativeRepoMapFileOwnerProjectionV1>, NativeRepoMapOwnerError> {
        self.compile_native_file_owners_impl_v1(bundle, None)
    }

    /// Compute ranks and materialize outputs only for the requested files.
    /// Whole-bundle validation and resource checks still include unselected
    /// rows. A caller must prove that its
    /// selected paths include every owner whose rank inputs or graph facts
    /// changed; a source-path edit alone is not such a proof because incoming
    /// edges and cross-file `OwnsChunk` edges can change another owner's rank.
    pub fn compile_native_selected_file_owners_v1(
        &self,
        bundle: &RepoMapSourceBundle,
        selected_paths: &BTreeSet<RepoRelativePath>,
    ) -> Result<Vec<NativeRepoMapFileOwnerProjectionV1>, NativeRepoMapOwnerError> {
        self.compile_native_file_owners_impl_v1(bundle, Some(selected_paths))
    }

    fn compile_native_file_owners_impl_v1(
        &self,
        bundle: &RepoMapSourceBundle,
        selected_paths: Option<&BTreeSet<RepoRelativePath>>,
    ) -> Result<Vec<NativeRepoMapFileOwnerProjectionV1>, NativeRepoMapOwnerError> {
        // A Source cut may lose its final file. Its successor has no file
        // contributions; the publication owner still advances separately.
        if bundle.nodes.is_empty() && bundle.edges.is_empty() {
            let _manifest_digest = producer_digest(&bundle.manifest_digest)
                .map_err(NativeRepoMapOwnerError::Compile)?;
            let _authority_digest = producer_digest(&bundle.authority_digest)
                .map_err(NativeRepoMapOwnerError::Compile)?;
            return Ok(Vec::new());
        }
        let mut file_nodes = BTreeMap::new();
        let (nodes, edges, rank_rows, commitments) =
            self.compile_native_parts_v1(bundle, selected_paths, Some(&mut file_nodes))?;
        let mut owners = BTreeMap::<&RepoRelativePath, NativeRepoMapOwnerProjection>::new();
        let mut owner_facts = BTreeMap::<&RepoRelativePath, Vec<RepoMapNode>>::new();
        // Nodes arrive in identity order. Each file's retained facts inherit
        // that order without cloning or sorting the other files' identities.
        for &node in file_nodes.values() {
            let path = node_owner_path(node);
            owner_facts.entry(path).or_default().push(node.clone());
            let owner = owners
                .entry(path)
                .or_insert_with(|| NativeRepoMapOwnerProjection {
                    output: GraphOwnerOutput {
                        nodes: Vec::new(),
                        edges: Vec::new(),
                        projection: Vec::new(),
                        facts: None,
                    },
                    rank_rows: Vec::new(),
                    commitments,
                });
            if owner.commitments != commitments {
                return Err(NativeRepoMapOwnerError::OwnerPartition);
            }
        }
        for node in nodes {
            let path = node_owner_path(
                file_nodes.get(&node.identity).ok_or(NativeRepoMapOwnerError::OwnerPartition)?,
            );
            owners
                .get_mut(path)
                .ok_or(NativeRepoMapOwnerError::OwnerPartition)?
                .output
                .nodes
                .push(node);
        }
        for edge in edges {
            // Whole-graph validation already certified the target. An edge
            // is owned by its source file even when the target is unselected.
            let path = node_owner_path(
                file_nodes.get(&edge.source).ok_or(NativeRepoMapOwnerError::OwnerPartition)?,
            );
            owners
                .get_mut(path)
                .ok_or(NativeRepoMapOwnerError::OwnerPartition)?
                .output
                .edges
                .push(edge);
        }
        for row in rank_rows {
            let path = node_owner_path(
                file_nodes.get(&row.subject).ok_or(NativeRepoMapOwnerError::OwnerPartition)?,
            );
            if path != &row.owner_path {
                return Err(NativeRepoMapOwnerError::OwnerPartition);
            }
            owners
                .get_mut(path)
                .ok_or(NativeRepoMapOwnerError::OwnerPartition)?
                .rank_rows
                .push(row);
        }
        let mut result = Vec::with_capacity(owners.len());
        for (owner_path, mut projection) in owners {
            let facts_nodes = owner_facts
                .remove(owner_path)
                .ok_or(NativeRepoMapOwnerError::OwnerPartition)?;
            projection
                .output
                .nodes
                .sort_unstable_by(|left, right| left.identity.cmp(&right.identity));
            projection.output.edges.sort_unstable_by(|left, right| {
                (left.kind.as_code_str(), &left.source, &left.target).cmp(&(
                    right.kind.as_code_str(),
                    &right.source,
                    &right.target,
                ))
            });
            projection
                .rank_rows
                .sort_unstable_by(|left, right| left.subject.cmp(&right.subject));
            // One additional direct child retains this file's typed facts.
            if projection.rank_rows.len() >= MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN {
                return Err(NativeRepoMapOwnerError::GraphLimit);
            }
            projection.output.projection = projection
                .rank_rows
                .iter()
                .map(|row| GraphProjectionRow {
                    subject: row.subject.clone(),
                    row: ObjectRef(Digest([0; 32])),
                })
                .collect();
            check_graph_encoding(&projection.output)?;
            result.push(NativeRepoMapFileOwnerProjectionV1 {
                owner_path: owner_path.clone(),
                projection,
                facts: NativeRepoMapFileFactsV1 {
                    owner_path: owner_path.clone(),
                    nodes: facts_nodes,
                },
            });
        }
        Ok(result)
    }

    /// Compile a source bundle with the same validation and ranking rules as
    /// the public RepoMap compiler. The caller supplies its source authority.
    pub fn compile_native_owner(
        &self,
        bundle: &RepoMapSourceBundle,
    ) -> Result<NativeRepoMapOwnerProjection, NativeRepoMapOwnerError> {
        if bundle.nodes.len().saturating_add(bundle.edges.len()) > MAX_NATIVE_GRAPH_ITEMS {
            return Err(NativeRepoMapOwnerError::GraphLimit);
        }
        let (nodes, edges, rank_rows, commitments) = self.compile_native_parts_v1(bundle, None, None)?;
        let output = GraphOwnerOutput {
            nodes,
            edges,
            projection: rank_rows
                .iter()
                .map(|rank| GraphProjectionRow {
                    subject: rank.subject.clone(),
                    row: ObjectRef(Digest([0; 32])),
                })
                .collect(),
            facts: None,
        };
        check_graph_encoding(&output)?;
        Ok(NativeRepoMapOwnerProjection {
            output,
            rank_rows,
            commitments,
        })
    }

    fn compile_native_parts_v1<'bundle>(
        &self,
        bundle: &'bundle RepoMapSourceBundle,
        selected_paths: Option<&BTreeSet<RepoRelativePath>>,
        mut file_nodes: Option<&mut BTreeMap<RepoMapNodeRef, &'bundle RepoMapNode>>,
    ) -> Result<
        (
            Vec<GraphNode>,
            Vec<GraphEdge>,
            Vec<CompiledRepoMapProjectionEntryV1>,
            RepoMapCandidateCommitmentsV1,
        ),
        NativeRepoMapOwnerError,
    > {
        let (graph, rank_rows, commitments, native_rank_valid) = self
            .compile_native_graph_parts(bundle, selected_paths)
            .map_err(NativeRepoMapOwnerError::Compile)?;
        if !native_rank_valid || rank_rows.iter().any(|row| !row.valid_native_rank_row()) {
            return Err(NativeRepoMapOwnerError::InvalidRankRow);
        }

        let mut nodes = if selected_paths.is_none() {
            Vec::with_capacity(bundle.nodes.len())
        } else {
            Vec::new()
        };
        for node in &bundle.nodes {
            if selected_paths.is_some_and(|selected| !selected.contains(node_owner_path(node))) {
                continue;
            }
            let identity = node_identity(node);
            if let Some(index) = file_nodes.as_deref_mut() {
                if index.insert(identity.clone(), node).is_some() {
                    return Err(NativeRepoMapOwnerError::OwnerPartition);
                }
            }
            nodes.push(GraphNode {
                identity,
                payload_digest: graph_node_payload_digest_v1(node)
                    .map_err(NativeRepoMapOwnerError::Encoding)?,
            });
        }
        let selected_sources = if selected_paths.is_some() {
            Some(file_nodes.ok_or(NativeRepoMapOwnerError::OwnerPartition)?)
        } else {
            None
        };
        nodes.sort_unstable_by(|left, right| left.identity.cmp(&right.identity));
        let mut edges = if selected_paths.is_none() {
            Vec::with_capacity(graph.edges.len())
        } else {
            Vec::new()
        };
        for edge in graph.edges {
            if selected_sources.as_ref().is_some_and(|index| !index.contains_key(&edge.source)) {
                continue;
            }
            edges.push(GraphEdge {
                kind: edge.kind,
                source: edge.source,
                target: edge.target,
            });
        }
        Ok((nodes, edges, rank_rows, commitments))
    }
}

fn graph_node_payload_digest_v1(node: &RepoMapNode) -> Result<Digest, serde_json::Error> {
    let mut hash = Sha256::new();
    hash.update(b"quanta-index/native-repomap-node/v1\0");
    serde_json::to_writer(&mut hash, node)?;
    Ok(Digest(hash.finalize().into()))
}

fn node_owner_path(node: &RepoMapNode) -> &RepoRelativePath {
    match node {
        RepoMapNode::File(file) => &file.repo_relative_path,
        RepoMapNode::Module(module) => &module.repo_relative_path,
        RepoMapNode::Symbol(symbol) => &symbol.owner_path,
        RepoMapNode::Chunk(chunk) => &chunk.owner_path,
    }
}

fn check_graph_encoding(output: &GraphOwnerOutput) -> Result<(), NativeRepoMapOwnerError> {
    if output.nodes.len().saturating_add(output.edges.len()) > MAX_NATIVE_GRAPH_ITEMS {
        return Err(NativeRepoMapOwnerError::GraphLimit);
    }
    struct Count(usize);
    impl io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            if self.0 > MAX_NATIVE_GRAPH_OUTPUT_BYTES {
                return Err(io::Error::other("graph owner encoded bytes exceed bound"));
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, output).map_err(|cause| {
        if count.0 > MAX_NATIVE_GRAPH_OUTPUT_BYTES {
            NativeRepoMapOwnerError::GraphLimit
        } else {
            NativeRepoMapOwnerError::Encoding(cause)
        }
    })
}
