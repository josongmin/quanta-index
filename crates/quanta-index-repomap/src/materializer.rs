//! P02A whole-bundle validated `RepoMap` graph compiler.
//!
//! Replaces the permissive `RepoMapMaterializer` transformer: every bundle is
//! fully validated (typed variant+domain identity, duplicate/dangling/illegal
//! edges, self-loop policy, budgets with overflow-safe accounting) before any
//! output exists. Failures are typed `RepoMapCompileRefusalV1` values that
//! never carry input payload bytes. The compiler performs zero durable
//! mutation and never calls object store, catalog, activation, quarantine, or
//! the global sequence allocator.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest as _, Sha256};

use quanta_index_contract::{
    ChunkId, CompiledRepoMapCandidateV1, CompiledRepoMapEdgeV1, CompiledRepoMapGraphV1,
    CompiledRepoMapNodeV1, CompiledRepoMapProjectionEntryV1, FileId, ManifestGeneration, RepoId,
    RepoMapCandidateCommitmentsV1, RepoMapChunkExactness, RepoMapChunkNode,
    RepoMapCompileRefusalCodeV1, RepoMapCompileRefusalV1, RepoMapCompileStageV1,
    RepoMapCompilerBudgetV1, RepoMapDocType, RepoMapEdge, RepoMapEdgeKind, RepoMapExactnessSummary,
    RepoMapFileNode, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapModuleId,
    RepoMapNode, RepoMapNodeRef, RepoMapRedactionState, RepoMapResourceReceiptV1,
    RepoMapSnapshotMeta, RepoMapSourceBundle, RepoMapSymbolNode, RepoRelativePath, RevisionId,
    SymbolId,
};

use crate::model::{RepoMapEntry, RepoMapSnapshot};

const COMPILED_GRAPH_DOMAIN: &str = "quanta-index/repomap-compiled-graph/v1";
const COMPILED_SCHEMA_DOMAIN: &str = "quanta-index/repomap-compiled-schema/v1";
const PROJECTION_PROFILE_DOMAIN: &str = "quanta-index/repomap-projection-profile/v1";

/// Whole-bundle validated compiler. Deterministic: the compiled commitment is
/// independent of input node/edge order because every table is canonically
/// sorted before encoding.
pub struct RepoMapGraphCompiler {
    budget: RepoMapCompilerBudgetV1,
}

#[derive(Clone, Debug, Default)]
struct ChunkStatsV1 {
    token_total: u32,
    preview_fragments: Vec<String>,
    preview_bytes: u64,
    exactness_markers: Vec<RepoMapChunkExactness>,
}

#[derive(Clone, Debug)]
struct FileEntryInput {
    subject: RepoMapNodeRef,
    owner_path: String,
    line_count: u32,
}

#[derive(Clone, Debug)]
struct SymbolEntryInput {
    subject: RepoMapNodeRef,
    owner_path: String,
    local_name: String,
    qualified_name: String,
    symbol_kind: String,
}

impl RepoMapGraphCompiler {
    #[must_use]
    pub const fn new(budget: RepoMapCompilerBudgetV1) -> Self {
        Self { budget }
    }

    #[must_use]
    pub const fn with_default_budget() -> Self {
        Self {
            budget: RepoMapCompilerBudgetV1::default_ceiling(),
        }
    }

    /// Compile a whole source bundle into the immutable P02A candidate.
    pub fn compile(
        &self,
        bundle: &RepoMapSourceBundle,
    ) -> Result<CompiledRepoMapCandidateV1, RepoMapCompileRefusalV1> {
        // Stage: BundleValidation -------------------------------------------
        if bundle.nodes.is_empty() {
            return Err(refusal(
                RepoMapCompileStageV1::BundleValidation,
                RepoMapCompileRefusalCodeV1::EmptyBundle,
                None,
                0,
            ));
        }
        let producer_manifest = producer_digest(bundle.manifest_digest.as_str())?;
        let producer_authority = producer_digest(bundle.authority_digest.as_str())?;

        // Stage: GraphValidation --------------------------------------------
        let identities = self.validated_identities(bundle)?;
        let canonical_edges = self.validated_edges(bundle, &identities)?;

        // Stage: Budget ------------------------------------------------------
        let node_count = u64::try_from(bundle.nodes.len()).map_err(|_error| overflow())?;
        let edge_count = u64::try_from(bundle.edges.len()).map_err(|_error| overflow())?;
        check_cap(
            RepoMapCompileRefusalCodeV1::NodeLimitExceeded,
            self.budget.max_nodes,
            node_count,
        )?;
        check_cap(
            RepoMapCompileRefusalCodeV1::EdgeLimitExceeded,
            self.budget.max_edges,
            edge_count,
        )?;
        let work_units = node_count
            .checked_add(edge_count)
            .and_then(|sum| sum.checked_mul(2))
            .ok_or_else(overflow)?;
        check_cap(
            RepoMapCompileRefusalCodeV1::WorkLimitExceeded,
            self.budget.max_work_units,
            work_units,
        )?;

        // Stage: Projection ---------------------------------------------------
        let projection = self.build_projection(bundle, &canonical_edges)?;

        // Stage: Commitment ---------------------------------------------------
        let canonical_graph = canonical_graph_table(&identities, &canonical_edges);
        let compiled_payload = encode_compiled_payload(&canonical_graph, &projection);
        let materialized_bytes =
            u64::try_from(compiled_payload.len()).map_err(|_error| overflow())?;
        check_cap(
            RepoMapCompileRefusalCodeV1::MaterializedByteLimitExceeded,
            self.budget.max_materialized_bytes,
            materialized_bytes,
        )?;
        let graph_bytes = encode_graph_bytes(&canonical_graph);
        let schema_bytes = REPOMAP_COMPILE_SCHEMA_DESCRIPTOR.as_bytes().to_vec();
        let profile_bytes = REPOMAP_COMPILE_PROJECTION_PROFILE_DESCRIPTOR
            .as_bytes()
            .to_vec();
        let owner_symbols = canonical_graph
            .nodes
            .iter()
            .filter(|node| matches!(node.identity, RepoMapNodeRef::Symbol(_)))
            .count();
        let owner_symbols = u64::try_from(owner_symbols).map_err(|_error| overflow())?;
        let commitments = RepoMapCandidateCommitmentsV1 {
            producer_manifest,
            producer_authority,
            compiled_graph: compile_domain_digest(COMPILED_GRAPH_DOMAIN, &graph_bytes),
            schema: compile_domain_digest(COMPILED_SCHEMA_DOMAIN, &schema_bytes),
            projection_profile: compile_domain_digest(PROJECTION_PROFILE_DOMAIN, &profile_bytes),
        };
        let resource_receipt = RepoMapResourceReceiptV1 {
            nodes: node_count,
            edges: edge_count,
            owner_symbols,
            preview_bytes: projection
                .iter()
                .map(|entry| u64::try_from(entry.search_text.len()).map_or(u64::MAX, |len| len))
                .fold(0_u64, u64::saturating_add),
            materialized_bytes,
            work_units,
            budget: self.budget,
        };
        debug_assert!(resource_receipt.observed());
        Ok(CompiledRepoMapCandidateV1::from_parts(
            canonical_graph,
            projection,
            commitments,
            compiled_payload,
            resource_receipt,
        ))
    }

    fn validated_identities(
        &self,
        bundle: &RepoMapSourceBundle,
    ) -> Result<BTreeSet<RepoMapNodeRef>, RepoMapCompileRefusalV1> {
        let mut identities = BTreeSet::new();
        let mut raw_id_variants = BTreeMap::<String, u8>::new();
        for node in &bundle.nodes {
            let identity = node_identity(node);
            let (raw_id, discriminant) = node_ref_parts(&identity);
            if !identities.insert(identity) {
                return Err(refusal(
                    RepoMapCompileStageV1::GraphValidation,
                    RepoMapCompileRefusalCodeV1::DuplicateNode,
                    None,
                    identities_len(&identities),
                ));
            }
            match raw_id_variants.insert(raw_id, discriminant) {
                Some(seen) if seen != discriminant => {
                    return Err(refusal(
                        RepoMapCompileStageV1::GraphValidation,
                        RepoMapCompileRefusalCodeV1::CrossVariantIdentityCollision,
                        None,
                        identities_len(&identities),
                    ));
                }
                _ => {}
            }
        }
        Ok(identities)
    }

    fn validated_edges(
        &self,
        bundle: &RepoMapSourceBundle,
        identities: &BTreeSet<RepoMapNodeRef>,
    ) -> Result<Vec<CompiledRepoMapEdgeV1>, RepoMapCompileRefusalV1> {
        let mut edges = Vec::new();
        for edge in &bundle.edges {
            let (kind, source, target) = match edge {
                RepoMapEdge::Contains(contains) => (
                    RepoMapEdgeKind::Contains,
                    &contains.container,
                    &contains.contained,
                ),
                RepoMapEdge::Call(call) => (RepoMapEdgeKind::Call, &call.caller, &call.callee),
                RepoMapEdge::Import(import) => {
                    (RepoMapEdgeKind::Import, &import.importer, &import.imported)
                }
                RepoMapEdge::OwnsChunk(owns) => {
                    (RepoMapEdgeKind::OwnsChunk, &owns.owner, &owns.chunk)
                }
                RepoMapEdge::DependsOn(depends) => (
                    RepoMapEdgeKind::DependsOn,
                    &depends.dependent,
                    &depends.dependency,
                ),
            };
            if !identities.contains(source) || !identities.contains(target) {
                return Err(refusal(
                    RepoMapCompileStageV1::GraphValidation,
                    RepoMapCompileRefusalCodeV1::DanglingEdgeEndpoint,
                    None,
                    edges_len(bundle),
                ));
            }
            if source == target {
                return Err(refusal(
                    RepoMapCompileStageV1::GraphValidation,
                    RepoMapCompileRefusalCodeV1::SelfLoop,
                    None,
                    edges_len(bundle),
                ));
            }
            if !edge_variant_legal(kind, source, target) {
                return Err(refusal(
                    RepoMapCompileStageV1::GraphValidation,
                    RepoMapCompileRefusalCodeV1::IllegalEdgeVariant,
                    None,
                    edges_len(bundle),
                ));
            }
            let canonical = CompiledRepoMapEdgeV1 {
                kind,
                source: source.clone(),
                target: target.clone(),
            };
            if edges.contains(&canonical) {
                return Err(refusal(
                    RepoMapCompileStageV1::GraphValidation,
                    RepoMapCompileRefusalCodeV1::DuplicateNode,
                    None,
                    edges_len(bundle),
                ));
            }
            edges.push(canonical);
        }
        Ok(edges)
    }

    fn build_projection(
        &self,
        bundle: &RepoMapSourceBundle,
        canonical_edges: &[CompiledRepoMapEdgeV1],
    ) -> Result<Vec<CompiledRepoMapProjectionEntryV1>, RepoMapCompileRefusalV1> {
        let mut degrees = BTreeMap::<&RepoMapNodeRef, (u32, u32)>::new();
        for edge in canonical_edges {
            degrees.entry(&edge.source).or_default().0 = degrees
                .get(&edge.source)
                .map_or(0, |pair| pair.0)
                .saturating_add(1);
            degrees.entry(&edge.target).or_default().1 = degrees
                .get(&edge.target)
                .map_or(0, |pair| pair.1)
                .saturating_add(1);
        }
        let mut per_owner_symbols = BTreeMap::<&str, u64>::new();
        let mut files = Vec::new();
        let mut symbols = Vec::new();
        for node in &bundle.nodes {
            match node {
                RepoMapNode::File(RepoMapFileNode {
                    file_id,
                    repo_relative_path,
                    line_count,
                }) => files.push(FileEntryInput {
                    subject: RepoMapNodeRef::File(file_id.clone()),
                    owner_path: repo_relative_path.as_str().to_string(),
                    line_count: *line_count,
                }),
                RepoMapNode::Symbol(RepoMapSymbolNode {
                    symbol_id,
                    owner_path,
                    local_name,
                    qualified_name,
                    symbol_kind,
                }) => {
                    let count = per_owner_symbols.entry(owner_path.as_str()).or_insert(0);
                    *count = count.checked_add(1).ok_or_else(overflow)?;
                    symbols.push(SymbolEntryInput {
                        subject: RepoMapNodeRef::Symbol(symbol_id.clone()),
                        owner_path: owner_path.as_str().to_string(),
                        local_name: local_name.clone(),
                        qualified_name: qualified_name.clone(),
                        symbol_kind: symbol_kind.as_str().to_string(),
                    });
                }
                RepoMapNode::Module(_) | RepoMapNode::Chunk(_) => {}
            }
        }
        for observed in per_owner_symbols.values() {
            if *observed > self.budget.max_owner_symbols {
                return Err(refusal(
                    RepoMapCompileStageV1::Projection,
                    RepoMapCompileRefusalCodeV1::OwnerSymbolLimitExceeded,
                    Some(self.budget.max_owner_symbols),
                    *observed,
                ));
            }
        }

        let chunk_stats = validated_chunk_stats(bundle)?;
        let mut projection = Vec::new();
        for file in &files {
            let search_text = file_search_text(file, &chunk_stats.0);
            let preview_bytes = u64::try_from(search_text.len()).map_err(|_error| overflow())?;
            check_cap(
                RepoMapCompileRefusalCodeV1::PreviewLimitExceeded,
                self.budget.max_preview_bytes,
                preview_bytes,
            )?;
            let degree = degrees.get(&file.subject).copied().unwrap_or((0, 0));
            let graph_degree = degree.0.saturating_add(degree.1);
            let final_score_millis = file_score_millis(file, &chunk_stats.0, graph_degree);
            projection.push(CompiledRepoMapProjectionEntryV1 {
                subject: file.subject.clone(),
                doc_type: RepoMapDocType::File,
                owner_path: quanta_index_contract::RepoRelativePath::new(file.owner_path.clone()),
                search_text,
                final_score_millis: i64::from(final_score_millis),
            });
        }
        for symbol in &symbols {
            let search_text = symbol_search_text(symbol, &chunk_stats.1);
            let preview_bytes = u64::try_from(search_text.len()).map_err(|_error| overflow())?;
            check_cap(
                RepoMapCompileRefusalCodeV1::PreviewLimitExceeded,
                self.budget.max_preview_bytes,
                preview_bytes,
            )?;
            let degree = degrees.get(&symbol.subject).copied().unwrap_or((0, 0));
            let graph_degree = degree.0.saturating_add(degree.1);
            let final_score_millis = symbol_score_millis(symbol, &chunk_stats.1, graph_degree);
            projection.push(CompiledRepoMapProjectionEntryV1 {
                subject: symbol.subject.clone(),
                doc_type: RepoMapDocType::Symbol,
                owner_path: quanta_index_contract::RepoRelativePath::new(symbol.owner_path.clone()),
                search_text,
                final_score_millis: i64::from(final_score_millis),
            });
        }
        projection.sort_by(|lhs, rhs| lhs.subject.cmp(&rhs.subject));
        Ok(projection)
    }
}

fn refusal(
    stage: RepoMapCompileStageV1,
    code: RepoMapCompileRefusalCodeV1,
    limit: Option<u64>,
    observed: u64,
) -> RepoMapCompileRefusalV1 {
    RepoMapCompileRefusalV1::new(stage, code, limit, observed)
}

fn overflow() -> RepoMapCompileRefusalV1 {
    refusal(
        RepoMapCompileStageV1::Budget,
        RepoMapCompileRefusalCodeV1::ArithmeticOverflow,
        None,
        u64::MAX,
    )
}

fn check_cap(
    code: RepoMapCompileRefusalCodeV1,
    limit: u64,
    observed: u64,
) -> Result<(), RepoMapCompileRefusalV1> {
    if observed > limit {
        return Err(refusal(
            RepoMapCompileStageV1::Budget,
            code,
            Some(limit),
            observed,
        ));
    }
    Ok(())
}

fn identities_len(identities: &BTreeSet<RepoMapNodeRef>) -> u64 {
    u64::try_from(identities.len()).map_or(u64::MAX, |len| len)
}

fn edges_len(bundle: &RepoMapSourceBundle) -> u64 {
    u64::try_from(bundle.edges.len()).map_or(u64::MAX, |len| len)
}

fn node_identity(node: &RepoMapNode) -> RepoMapNodeRef {
    match node {
        RepoMapNode::File(inner) => RepoMapNodeRef::File(inner.file_id.clone()),
        RepoMapNode::Module(inner) => RepoMapNodeRef::Module(inner.module_id.clone()),
        RepoMapNode::Symbol(inner) => RepoMapNodeRef::Symbol(inner.symbol_id.clone()),
        RepoMapNode::Chunk(inner) => RepoMapNodeRef::Chunk(inner.chunk_id.clone()),
    }
}

fn node_ref_parts(node_ref: &RepoMapNodeRef) -> (String, u8) {
    match node_ref {
        RepoMapNodeRef::File(id) => (id.as_str().to_string(), 0),
        RepoMapNodeRef::Module(id) => (id.as_str().to_string(), 1),
        RepoMapNodeRef::Symbol(id) => (id.as_str().to_string(), 2),
        RepoMapNodeRef::Chunk(id) => (id.as_str().to_string(), 3),
    }
}

fn edge_variant_legal(
    kind: RepoMapEdgeKind,
    source: &RepoMapNodeRef,
    target: &RepoMapNodeRef,
) -> bool {
    match kind {
        RepoMapEdgeKind::Contains => {
            matches!(source, RepoMapNodeRef::File(_) | RepoMapNodeRef::Module(_))
                && !matches!(target, RepoMapNodeRef::File(_))
        }
        RepoMapEdgeKind::Call => {
            matches!(source, RepoMapNodeRef::Symbol(_))
                && matches!(target, RepoMapNodeRef::Symbol(_))
        }
        RepoMapEdgeKind::Import => {
            matches!(source, RepoMapNodeRef::File(_) | RepoMapNodeRef::Module(_))
                && matches!(target, RepoMapNodeRef::File(_) | RepoMapNodeRef::Module(_))
        }
        RepoMapEdgeKind::OwnsChunk => {
            matches!(target, RepoMapNodeRef::Chunk(_))
                && matches!(
                    source,
                    RepoMapNodeRef::File(_) | RepoMapNodeRef::Module(_) | RepoMapNodeRef::Symbol(_)
                )
        }
        RepoMapEdgeKind::DependsOn => {
            !matches!(source, RepoMapNodeRef::Chunk(_))
                && !matches!(target, RepoMapNodeRef::Chunk(_))
        }
    }
}

fn producer_digest(text: &str) -> Result<[u8; 32], RepoMapCompileRefusalV1> {
    let hex = text.strip_prefix("sha256:").unwrap_or(text);
    let mut bytes = [0_u8; 32];
    if hex_decode32(hex, &mut bytes) {
        Ok(bytes)
    } else {
        Err(refusal(
            RepoMapCompileStageV1::BundleValidation,
            RepoMapCompileRefusalCodeV1::ProducerDigestInvalid,
            None,
            u64::try_from(hex.len()).map_or(u64::MAX, |len| len),
        ))
    }
}

fn hex_decode32(hex: &str, out: &mut [u8; 32]) -> bool {
    let hex_bytes = hex.as_bytes();
    if hex_bytes.len() != 64 {
        return false;
    }
    for (index, pair) in hex_bytes.chunks_exact(2).enumerate() {
        let (Some(high_nibble), Some(low_nibble)) = (pair.first().copied(), pair.get(1).copied())
        else {
            return false;
        };
        match (hex_nibble(high_nibble), hex_nibble(low_nibble)) {
            (Some(high), Some(low)) => {
                let Some(slot) = out.get_mut(index) else {
                    return false;
                };
                *slot = (high << 4) | low;
            }
            _ => return false,
        }
    }
    true
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte.saturating_sub(b'0')),
        b'a'..=b'f' => Some(byte.saturating_sub(b'a').saturating_add(10)),
        _ => None,
    }
}

/// Validated chunk statistics: (`by_owner_path`, `by_subject_ref`). Dangling
/// `OwnsChunk` endpoints were already refused by the edge validation stage.
type ChunkStatsTables = (
    BTreeMap<String, ChunkStatsV1>,
    BTreeMap<String, ChunkStatsV1>,
);

fn validated_chunk_stats(
    bundle: &RepoMapSourceBundle,
) -> Result<ChunkStatsTables, RepoMapCompileRefusalV1> {
    let mut by_owner = BTreeMap::<String, ChunkStatsV1>::new();
    let mut by_subject = BTreeMap::<String, ChunkStatsV1>::new();
    let mut chunks = BTreeMap::<&str, &RepoMapChunkNode>::new();
    for node in &bundle.nodes {
        if let RepoMapNode::Chunk(chunk) = node {
            let inserted = chunks.insert(chunk.chunk_id.as_str(), chunk);
            debug_assert!(
                inserted.is_none(),
                "duplicate chunk identities were already refused"
            );
        }
    }
    for chunk in chunks.values() {
        accumulate_chunk(
            by_owner
                .entry(chunk.owner_path.as_str().to_string())
                .or_default(),
            chunk,
        )?;
    }
    for edge in &bundle.edges {
        if let RepoMapEdge::OwnsChunk(owns) = edge {
            let RepoMapNodeRef::Chunk(chunk_id) = &owns.chunk else {
                continue;
            };
            if let Some(chunk) = chunks.get(chunk_id.as_str()) {
                accumulate_chunk(
                    by_subject.entry(node_ref_key(&owns.owner)).or_default(),
                    chunk,
                )?;
            }
        }
    }
    Ok((by_owner, by_subject))
}

fn accumulate_chunk(
    stats: &mut ChunkStatsV1,
    chunk: &RepoMapChunkNode,
) -> Result<(), RepoMapCompileRefusalV1> {
    stats.token_total = stats
        .token_total
        .checked_add(chunk.token_count)
        .ok_or_else(overflow)?;
    let trimmed = chunk.preview_text.trim();
    if !trimmed.is_empty() {
        let len = u64::try_from(trimmed.len()).map_err(|_error| overflow())?;
        stats.preview_bytes = stats.preview_bytes.checked_add(len).ok_or_else(overflow)?;
        stats.preview_fragments.push(trimmed.to_string());
    }
    stats.exactness_markers.push(chunk.exactness);
    Ok(())
}

fn node_ref_key(node_ref: &RepoMapNodeRef) -> String {
    let (raw, discriminant) = node_ref_parts(node_ref);
    format!("{discriminant}:{raw}")
}

fn file_search_text(
    file: &FileEntryInput,
    owner_chunks: &BTreeMap<String, ChunkStatsV1>,
) -> String {
    let chunks = owner_chunks.get(file.owner_path.as_str());
    let mut parts = vec![
        node_ref_key(&file.subject),
        file.owner_path.clone(),
        "file".to_string(),
    ];
    if let Some(chunks) = chunks {
        parts.push(chunks.preview_fragments.join(" "));
    }
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn symbol_search_text(
    symbol: &SymbolEntryInput,
    subject_chunks: &BTreeMap<String, ChunkStatsV1>,
) -> String {
    let chunks = subject_chunks.get(&node_ref_key(&symbol.subject));
    let mut parts = vec![
        node_ref_key(&symbol.subject),
        symbol.local_name.clone(),
        symbol.qualified_name.clone(),
        symbol.symbol_kind.clone(),
        symbol.owner_path.clone(),
    ];
    if let Some(chunks) = chunks {
        parts.push(chunks.preview_fragments.join(" "));
    }
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn file_score_millis(
    file: &FileEntryInput,
    owner_chunks: &BTreeMap<String, ChunkStatsV1>,
    graph_degree: u32,
) -> u32 {
    let chunk_total = owner_chunks
        .get(file.owner_path.as_str())
        .map_or(0, |stats| stats.token_total);
    let importance = (280_u32)
        .saturating_add(graph_degree.saturating_mul(28))
        .saturating_add(file.line_count.div_euclid(8).min(120));
    let utility = (240_u32).saturating_add(chunk_total.div_euclid(3).min(260));
    let final_millis = importance
        .saturating_mul(4)
        .saturating_add(utility.saturating_mul(3))
        .div_euclid(7);
    final_millis.min(1_000)
}

fn symbol_score_millis(
    symbol: &SymbolEntryInput,
    subject_chunks: &BTreeMap<String, ChunkStatsV1>,
    graph_degree: u32,
) -> u32 {
    let chunk_total = subject_chunks
        .get(&node_ref_key(&symbol.subject))
        .map_or(0, |stats| stats.token_total);
    let importance = (420_u32).saturating_add(graph_degree.saturating_mul(35));
    let utility = (320_u32).saturating_add(chunk_total.div_euclid(4).min(220));
    let final_millis = importance
        .saturating_mul(4)
        .saturating_add(utility.saturating_mul(3))
        .div_euclid(7);
    final_millis.min(1_000)
}

fn canonical_graph_table(
    identities: &BTreeSet<RepoMapNodeRef>,
    edges: &[CompiledRepoMapEdgeV1],
) -> CompiledRepoMapGraphV1 {
    let mut degrees = BTreeMap::<&RepoMapNodeRef, (u32, u32)>::new();
    for edge in edges {
        let source = degrees.entry(&edge.source).or_default();
        source.0 = source.0.saturating_add(1);
        let target = degrees.entry(&edge.target).or_default();
        target.1 = target.1.saturating_add(1);
    }
    let nodes = identities
        .iter()
        .map(|identity| {
            let (degree_in, degree_out) = degrees.get(identity).copied().unwrap_or((0, 0));
            CompiledRepoMapNodeV1 {
                identity: identity.clone(),
                degree_in,
                degree_out,
            }
        })
        .collect();
    let mut edges = edges.to_vec();
    edges.sort_by(|lhs, rhs| {
        lhs.kind
            .as_code_str()
            .cmp(rhs.kind.as_code_str())
            .then(lhs.source.cmp(&rhs.source))
            .then(lhs.target.cmp(&rhs.target))
    });
    CompiledRepoMapGraphV1 { nodes, edges }
}

fn encode_graph_bytes(graph: &CompiledRepoMapGraphV1) -> Vec<u8> {
    let mut writer = CompileCborWriter::new();
    writer.write_array_header(graph.nodes.len());
    for node in &graph.nodes {
        writer.write_array_header(3);
        write_node_ref(&mut writer, &node.identity);
        writer.write_u64(u64::from(node.degree_in));
        writer.write_u64(u64::from(node.degree_out));
    }
    writer.write_array_header(graph.edges.len());
    for edge in &graph.edges {
        writer.write_array_header(3);
        writer.write_text(edge.kind.as_code_str());
        write_node_ref(&mut writer, &edge.source);
        write_node_ref(&mut writer, &edge.target);
    }
    writer.into_bytes()
}

fn encode_compiled_payload(
    graph: &CompiledRepoMapGraphV1,
    projection: &[CompiledRepoMapProjectionEntryV1],
) -> Vec<u8> {
    let mut writer = CompileCborWriter::new();
    writer.write_array_header(2);
    writer.write_array_header(graph.nodes.len());
    for node in &graph.nodes {
        writer.write_array_header(3);
        write_node_ref(&mut writer, &node.identity);
        writer.write_u64(u64::from(node.degree_in));
        writer.write_u64(u64::from(node.degree_out));
    }
    writer.write_array_header(graph.edges.len());
    for edge in &graph.edges {
        writer.write_array_header(3);
        writer.write_text(edge.kind.as_code_str());
        write_node_ref(&mut writer, &edge.source);
        write_node_ref(&mut writer, &edge.target);
    }
    writer.write_array_header(projection.len());
    for entry in projection {
        writer.write_array_header(5);
        write_node_ref(&mut writer, &entry.subject);
        writer.write_text(entry.doc_type.as_code_str());
        writer.write_text(entry.owner_path.as_str());
        writer.write_text(&entry.search_text);
        writer.write_i64(entry.final_score_millis);
    }
    writer.into_bytes()
}

/// Legacy store-boundary adapter. Runs the full compiler validation and
/// refuses typed; P03 replaces this consumer with the sealed candidate path.
pub struct RepoMapMaterializer;

impl RepoMapMaterializer {
    pub fn materialize(
        bundle: &RepoMapSourceBundle,
    ) -> Result<RepoMapSnapshot, RepoMapCompileRefusalV1> {
        compile_snapshot(bundle)
    }
}

/// Legacy snapshot materialization entry retained for the store boundary.
///
/// It runs the full compiler validation first and refuses typed instead of
/// producing a partial snapshot. P03 replaces this consumer with the sealed
/// candidate path.
pub fn compile_snapshot(
    bundle: &RepoMapSourceBundle,
) -> Result<RepoMapSnapshot, RepoMapCompileRefusalV1> {
    let compiler = RepoMapGraphCompiler::with_default_budget();
    let candidate = compiler.compile(bundle)?;
    Ok(snapshot_from_projection(
        &bundle.repo_id,
        &bundle.revision_id,
        bundle.manifest_generation,
        &CandidateProjectionMetaV1::from_bundle(bundle),
        candidate.projection(),
    ))
}

/// The bundle-declared projection metadata a sealed candidate's query
/// projection is rebuilt from.
///
/// The P03 store persists exactly this (as JSON in the catalog candidate
/// row) so the boot-rebuilt snapshot is byte-identical to the publish-time
/// one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateProjectionMetaV1 {
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    /// Present for candidates sealed by the current store owner. Legacy
    /// catalog rows remain readable but cannot satisfy V2 activation.
    pub manifest_digest: Option<String>,
    pub source_bundle_digest: Option<String>,
    pub item_index_availability: RepoMapItemIndexAvailability,
    pub graph_coverage_class: RepoMapGraphCoverageClass,
    pub exactness_summary: RepoMapExactnessSummary,
    pub redaction_state: RepoMapRedactionState,
}

impl CandidateProjectionMetaV1 {
    #[must_use]
    pub fn from_bundle(bundle: &RepoMapSourceBundle) -> Self {
        Self {
            snapshot_id: bundle.snapshot_id.clone(),
            projection_version: bundle.projection_version,
            authority_digest: bundle.authority_digest.clone(),
            manifest_digest: None,
            source_bundle_digest: None,
            item_index_availability: bundle.graph_coverage.item_index_availability,
            graph_coverage_class: bundle.graph_coverage.graph_coverage_class,
            exactness_summary: bundle.exactness_summary,
            redaction_state: bundle.redaction_state,
        }
    }

    #[must_use]
    pub fn from_bundle_with_source_digest_v2(
        bundle: &RepoMapSourceBundle,
        source_bundle_digest: String,
    ) -> Self {
        let mut meta = Self::from_bundle(bundle);
        meta.manifest_digest = Some(bundle.manifest_digest.clone());
        meta.source_bundle_digest = Some(source_bundle_digest);
        meta
    }

    /// Canonical JSON column form. Enum fields use their wire strings.
    pub fn to_json(&self) -> Result<String, quanta_index_core::CoreError> {
        let value = match (&self.manifest_digest, &self.source_bundle_digest) {
            (None, None) => serde_json::json!({
                "snapshot_id": self.snapshot_id,
                "projection_version": self.projection_version,
                "authority_digest": self.authority_digest,
                "item_index_availability": self.item_index_availability.as_code_str(),
                "graph_coverage_class": self.graph_coverage_class.as_code_str(),
                "exactness_summary": self.exactness_summary.as_code_str(),
                "redaction_state": self.redaction_state.as_code_str(),
            }),
            (Some(manifest_digest), Some(source_bundle_digest)) => serde_json::json!({
                "snapshot_id": self.snapshot_id,
                "projection_version": self.projection_version,
                "authority_digest": self.authority_digest,
                "manifest_digest": manifest_digest,
                "source_bundle_digest": source_bundle_digest,
                "item_index_availability": self.item_index_availability.as_code_str(),
                "graph_coverage_class": self.graph_coverage_class.as_code_str(),
                "exactness_summary": self.exactness_summary.as_code_str(),
                "redaction_state": self.redaction_state.as_code_str(),
            }),
            _ => {
                return Err(quanta_index_core::CoreError::Storage(
                    "repomap projection meta strong custody fields must be both present or both absent"
                        .to_string(),
                ));
            }
        };
        serde_json::to_string(&value).map_err(|err| {
            quanta_index_core::CoreError::Storage(format!(
                "repomap projection meta encode failed: {err}"
            ))
        })
    }

    /// Strict parse of the JSON column form; unknown enum codes are
    /// refused typed, never defaulted.
    pub fn from_json(value: &str) -> Result<Self, quanta_index_core::CoreError> {
        fn enum_str<T: for<'de> serde::Deserialize<'de>>(
            value: &serde_json::Value,
            field: &str,
        ) -> Result<T, quanta_index_core::CoreError> {
            serde_json::from_value(value.get(field).cloned().ok_or_else(|| {
                quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: format!("repomap projection meta is missing `{field}`"),
                }
            })?)
            .map_err(|err| quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!("repomap projection meta field `{field}` is invalid: {err}"),
            })
        }
        fn plain_str(
            value: &serde_json::Value,
            field: &str,
        ) -> Result<String, quanta_index_core::CoreError> {
            value
                .get(field)
                .and_then(|field| field.as_str())
                .map(str::to_owned)
                .ok_or_else(|| quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: format!("repomap projection meta is missing string `{field}`"),
                })
        }
        fn optional_plain_str(
            value: &serde_json::Value,
            field: &str,
        ) -> Result<Option<String>, quanta_index_core::CoreError> {
            let Some(field_value) = value.get(field) else {
                return Ok(None);
            };
            field_value
                .as_str()
                .map(|text| Some(text.to_owned()))
                .ok_or_else(|| quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: format!(
                        "repomap projection meta field `{field}` is present but not a string"
                    ),
                })
        }
        let value: serde_json::Value =
            serde_json::from_str(value).map_err(|err| quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!("repomap projection meta is not valid JSON: {err}"),
            })?;
        let Some(Ok(projection_version)) = value
            .get("projection_version")
            .and_then(serde_json::Value::as_u64)
            .map(u32::try_from)
        else {
            return Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: "repomap projection meta is missing `projection_version`".to_string(),
            });
        };
        let manifest_digest = optional_plain_str(&value, "manifest_digest")?;
        let source_bundle_digest = optional_plain_str(&value, "source_bundle_digest")?;
        if manifest_digest.is_some() != source_bundle_digest.is_some() {
            return Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: "repomap projection meta has a partial V2 strong-custody binding"
                    .to_string(),
            });
        }
        if let (Some(manifest_digest), Some(source_bundle_digest)) =
            (&manifest_digest, &source_bundle_digest)
        {
            let _validated_manifest_digest = producer_digest(manifest_digest).map_err(|_| {
                quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: "repomap projection meta has an invalid V2 manifest custody digest"
                        .to_string(),
                }
            })?;
            let source_hex = source_bundle_digest
                .strip_prefix("sha256:")
                .ok_or_else(|| quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: "repomap projection meta has a non-canonical V2 source-bundle digest"
                        .to_string(),
                })?;
            if source_hex.len() != 64
                || !source_hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: "repomap projection meta has a non-canonical V2 source-bundle digest"
                        .to_string(),
                });
            }
        }
        Ok(Self {
            snapshot_id: plain_str(&value, "snapshot_id")?,
            projection_version,
            authority_digest: plain_str(&value, "authority_digest")?,
            manifest_digest,
            source_bundle_digest,
            item_index_availability: enum_str(&value, "item_index_availability")?,
            graph_coverage_class: enum_str(&value, "graph_coverage_class")?,
            exactness_summary: enum_str(&value, "exactness_summary")?,
            redaction_state: enum_str(&value, "redaction_state")?,
        })
    }
}

/// Build the query snapshot from a candidate projection plus its persisted
/// meta. Deterministic: the same projection and meta always build the same
/// snapshot, at publish time and at boot alike.
#[must_use]
pub fn snapshot_from_projection(
    repo_id: &RepoId,
    revision_id: &RevisionId,
    manifest_generation: ManifestGeneration,
    meta: &CandidateProjectionMetaV1,
    projection: &[CompiledRepoMapProjectionEntryV1],
) -> RepoMapSnapshot {
    let snapshot_meta = RepoMapSnapshotMeta {
        snapshot_id: meta.snapshot_id.clone(),
        projection_version: meta.projection_version,
        authority_digest: meta.authority_digest.clone(),
        item_index_availability: meta.item_index_availability,
        graph_coverage_class: meta.graph_coverage_class,
        exactness_summary: meta.exactness_summary,
    };
    let entries = projection
        .iter()
        .map(|entry| projection_entry_to_model_entry(meta, entry))
        .collect();
    RepoMapSnapshot {
        repo_id: repo_id.clone(),
        revision_id: revision_id.clone(),
        manifest_generation,
        snapshot_meta,
        entries,
    }
}

/// Strictly decode the compiled candidate payload back into its canonical
/// graph and projection tables.
///
/// Exactly one accepted form: the compiler's own canonical encoding; trailing
/// bytes, indefinite lengths, non-minimal integers and unknown variant tags
/// are refused.
pub fn decode_compiled_payload(
    bytes: &[u8],
) -> Result<
    (
        CompiledRepoMapGraphV1,
        Vec<CompiledRepoMapProjectionEntryV1>,
    ),
    quanta_index_core::CoreError,
> {
    let refuse = |detail: String| -> quanta_index_core::CoreError {
        quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: format!("repomap compiled payload decode refused: {detail}"),
        }
    };
    let mut reader = CompileCborReader::new(bytes);
    // The compiler's payload frame is: a top-level array header whose
    // length is the schema version (2), then the three canonical tables
    // (nodes, edges, projection) each under its own definite array header.
    let version = reader.array_header("payload").map_err(refuse)?;
    if version != 2 {
        return Err(refuse(format!(
            "payload schema version {version}, expected 2"
        )));
    }
    let nodes_len = reader.array_header("nodes").map_err(refuse)?;
    if !matches!(nodes_len, 0..=2_000_000) {
        return Err(refuse(format!("node table holds {nodes_len} entries")));
    }
    let mut nodes = Vec::new();
    for _ in 0..nodes_len {
        let entry_len = reader.array_header("node").map_err(refuse)?;
        if entry_len != 3 {
            return Err(refuse(format!(
                "node array holds {entry_len} items, expected 3"
            )));
        }
        let identity = reader.node_ref("node.identity").map_err(refuse)?;
        let degree_in = reader.u32_value("node.degree_in").map_err(refuse)?;
        let degree_out = reader.u32_value("node.degree_out").map_err(refuse)?;
        nodes.push(CompiledRepoMapNodeV1 {
            identity,
            degree_in,
            degree_out,
        });
    }
    let edges_len = reader.array_header("edges").map_err(refuse)?;
    if !matches!(edges_len, 0..=20_000_000) {
        return Err(refuse(format!("edge table holds {edges_len} entries")));
    }
    let mut edges = Vec::new();
    for _ in 0..edges_len {
        let entry_len = reader.array_header("edge").map_err(refuse)?;
        if entry_len != 3 {
            return Err(refuse(format!(
                "edge array holds {entry_len} items, expected 3"
            )));
        }
        let kind = reader.edge_kind("edge.kind").map_err(refuse)?;
        let source = reader.node_ref("edge.source").map_err(refuse)?;
        let target = reader.node_ref("edge.target").map_err(refuse)?;
        edges.push(CompiledRepoMapEdgeV1 {
            kind,
            source,
            target,
        });
    }
    let projection_len = reader.array_header("projection").map_err(refuse)?;
    if !matches!(projection_len, 0..=2_000_000) {
        return Err(refuse(format!(
            "projection table holds {projection_len} entries"
        )));
    }
    let mut projection = Vec::new();
    for _ in 0..projection_len {
        let entry_len = reader.array_header("projection-entry").map_err(refuse)?;
        if entry_len != 5 {
            return Err(refuse(format!(
                "projection entry holds {entry_len} items, expected 5"
            )));
        }
        let subject = reader.node_ref("projection.subject").map_err(refuse)?;
        let doc_type = reader.doc_type("projection.doc_type").map_err(refuse)?;
        let owner_path = reader.path_id("projection.owner_path").map_err(refuse)?;
        let search_text = reader.text("projection.search_text").map_err(refuse)?;
        let final_score_millis = reader
            .i64_value("projection.final_score_millis")
            .map_err(refuse)?;
        projection.push(CompiledRepoMapProjectionEntryV1 {
            subject,
            doc_type,
            owner_path,
            search_text,
            final_score_millis,
        });
    }
    reader.expect_end().map_err(refuse)?;
    Ok((CompiledRepoMapGraphV1 { nodes, edges }, projection))
}

/// Minimal strict CBOR reader for the compiler's canonical payload schema:
/// definite lengths, minimal integers, UTF-8 text only.
struct CompileCborReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> CompileCborReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn peek(&self) -> Result<u8, String> {
        self.bytes
            .get(self.position)
            .copied()
            .ok_or_else(|| "unexpected end".to_string())
    }

    fn take(&mut self) -> Result<u8, String> {
        let byte = self.peek()?;
        self.position = self.position.saturating_add(1);
        Ok(byte)
    }

    fn uint_value(&mut self, label: &str) -> Result<u64, String> {
        let major = self.take()?;
        let info = major & 0x1f;
        if major & 0xe0 != 0 {
            return Err(format!("{label}: expected unsigned integer"));
        }
        let width = match info {
            0..=23 => {
                let value = u64::from(info);
                return Ok(value);
            }
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return Err(format!("{label}: non-minimal or unsupported integer")),
        };
        let mut raw = [0_u8; 8];
        let end = self.position.saturating_add(width);
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| format!("{label}: unexpected end"))?;
        let offset = 8_usize
            .checked_sub(width)
            .ok_or_else(|| format!("{label}: width out of range"))?;
        raw.get_mut(offset..)
            .ok_or_else(|| format!("{label}: width out of range"))?
            .copy_from_slice(slice);
        self.position = end;
        let value = u64::from_be_bytes(raw);
        // Minimality: the encoding must be the shortest for its value.
        let minimal_shift = 8_usize.checked_mul(width.saturating_sub(1)).unwrap_or(0);
        if width < 8 && value < (1_u64 << minimal_shift) {
            return Err(format!("{label}: non-minimal integer"));
        }
        Ok(value)
    }

    /// The compiler's framing writes table and tuple lengths as bare
    /// canonical unsigned integers (not CBOR array heads); read exactly
    /// that, with the minimality check `uint_value` carries.
    fn array_header(&mut self, label: &str) -> Result<u64, String> {
        let value = self.uint_value(label)?;
        Ok(value)
    }

    fn uint_body(&mut self, info: u8, label: &str) -> Result<u64, String> {
        let width = match info {
            0..=23 => return Ok(u64::from(info)),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return Err(format!("{label}: unsupported array length")),
        };
        let mut raw = [0_u8; 8];
        let end = self.position.saturating_add(width);
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| format!("{label}: unexpected end"))?;
        let offset = 8_usize
            .checked_sub(width)
            .ok_or_else(|| format!("{label}: width out of range"))?;
        raw.get_mut(offset..)
            .ok_or_else(|| format!("{label}: width out of range"))?
            .copy_from_slice(slice);
        self.position = end;
        Ok(u64::from_be_bytes(raw))
    }

    fn text(&mut self, label: &str) -> Result<String, String> {
        let major = self.take()?;
        if (major >> 5) != 3 {
            return Err(format!("{label}: expected text"));
        }
        let len = self.uint_body(major & 0x1f, label)?;
        let len = usize::try_from(len).map_err(|_error| format!("{label}: length out of range"))?;
        let end = self.position.saturating_add(len);
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| format!("{label}: unexpected end"))?;
        self.position = end;
        std::str::from_utf8(slice)
            .map(str::to_owned)
            .map_err(|_error| format!("{label}: invalid UTF-8"))
    }

    fn u32_value(&mut self, label: &str) -> Result<u32, String> {
        let value = self.uint_value(label)?;
        u32::try_from(value).map_err(|_error| format!("{label}: does not fit u32"))
    }

    fn i64_value(&mut self, label: &str) -> Result<i64, String> {
        let major = self.peek()?;
        if major & 0xe0 == 0x20 {
            let _tag: u8 = self.take()?;
            let magnitude = self.uint_value(label)?;
            let signed = magnitude
                .checked_add(1)
                .ok_or_else(|| format!("{label}: overflow"))?;
            let signed = i64::try_from(signed).map_err(|_error| format!("{label}: overflow"))?;
            signed
                .checked_neg()
                .ok_or_else(|| format!("{label}: overflow"))
        } else {
            let value = self.uint_value(label)?;
            i64::try_from(value).map_err(|_error| format!("{label}: does not fit i64"))
        }
    }

    fn node_ref(&mut self, label: &str) -> Result<RepoMapNodeRef, String> {
        let entry_len = self.array_header(label)?;
        if entry_len != 2 {
            return Err(format!(
                "{label}: array holds {entry_len} items, expected 2"
            ));
        }
        let variant = self.text(label)?;
        let id = self.text(label)?;
        match variant.as_str() {
            "File" => Ok(RepoMapNodeRef::File(FileId::new(&id))),
            "Module" => Ok(RepoMapNodeRef::Module(RepoMapModuleId::new(&id))),
            "Symbol" => Ok(RepoMapNodeRef::Symbol(SymbolId::new(&id))),
            "Chunk" => Ok(RepoMapNodeRef::Chunk(ChunkId::new(&id))),
            other => Err(format!("{label}: unknown node variant `{other}`")),
        }
    }

    fn edge_kind(&mut self, label: &str) -> Result<RepoMapEdgeKind, String> {
        let code = self.text(label)?;
        serde_json::from_value(serde_json::Value::String(code))
            .map_err(|error| format!("{label}: {error}"))
    }

    fn doc_type(&mut self, label: &str) -> Result<RepoMapDocType, String> {
        let code = self.text(label)?;
        serde_json::from_value(serde_json::Value::String(code))
            .map_err(|error| format!("{label}: {error}"))
    }

    fn path_id(&mut self, label: &str) -> Result<RepoRelativePath, String> {
        let text = self.text(label)?;
        Ok(RepoRelativePath::new(&text))
    }

    fn expect_end(&self) -> Result<(), String> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err("trailing bytes".to_string())
        }
    }
}

fn projection_entry_to_model_entry(
    meta: &CandidateProjectionMetaV1,
    entry: &CompiledRepoMapProjectionEntryV1,
) -> RepoMapEntry {
    let final_score_millis = u32::try_from(entry.final_score_millis.clamp(0, 1_000))
        .map_or(0, |final_score_millis| final_score_millis);
    let (raw_identity, _discriminant) = node_ref_parts(&entry.subject);
    RepoMapEntry {
        subject_identity: raw_identity,
        subject_doc_type: entry.doc_type,
        subject_kind: if entry.doc_type == RepoMapDocType::File {
            "file".to_string()
        } else {
            "symbol".to_string()
        },
        owner_path: entry.owner_path.as_str().to_string(),
        score: f32::from(
            u16::try_from(final_score_millis.min(u32::from(u16::MAX))).map_or(0, u16::from),
        ) / 1_000.0,
        final_score_millis,
        importance_score_millis: final_score_millis,
        utility_score_millis: final_score_millis.div_euclid(2),
        freshness_score_millis: freshness_score(meta),
        evidence_priority_millis: final_score_millis.div_euclid(2),
        token_budget_hint: 64,
        contributing_signals: BTreeMap::new(),
        projection_evidence_kind: "CompiledRepoMapCandidateV1".to_string(),
        projection_authority_artifact_id: format!(
            "repo-map:{}:{}",
            meta.snapshot_id,
            node_ref_key(&entry.subject)
        ),
        projection_authority_digest: meta.authority_digest.clone(),
        projection_status: projection_status(meta),
        redaction_state: meta.redaction_state,
        search_text: entry.search_text.clone(),
        source_symbol_count: 0,
        source_chunk_token_total: 0,
        source_call_incoming_edges: 0,
        source_call_outgoing_edges: 0,
        source_import_incoming_edges: 0,
        source_import_outgoing_edges: 0,
    }
}

fn freshness_score(meta: &CandidateProjectionMetaV1) -> u32 {
    if matches!(
        meta.item_index_availability,
        RepoMapItemIndexAvailability::Available | RepoMapItemIndexAvailability::Full
    ) && matches!(
        meta.graph_coverage_class,
        RepoMapGraphCoverageClass::Complete | RepoMapGraphCoverageClass::Full
    ) {
        return 860;
    }
    500
}

fn projection_status(meta: &CandidateProjectionMetaV1) -> String {
    if matches!(
        meta.item_index_availability,
        RepoMapItemIndexAvailability::Available | RepoMapItemIndexAvailability::Full
    ) && matches!(
        meta.graph_coverage_class,
        RepoMapGraphCoverageClass::Complete | RepoMapGraphCoverageClass::Full
    ) {
        return "Complete".to_string();
    }
    "Partial".to_string()
}

// ---------------------------------------------------------------------------
// Private canonical CBOR writer and digest helpers (compiler-owned schema).
// ---------------------------------------------------------------------------

const REPOMAP_COMPILE_SCHEMA_DESCRIPTOR: &str = "quanta-index/repomap-compiled-schema/v1{nodes:identity,degree_in,degree_out;edges:kind,source,target;projection:subject,doc_type,owner_path,search_text,final_score_millis}";

const REPOMAP_COMPILE_PROJECTION_PROFILE_DESCRIPTOR: &str =
    "quanta-index/repomap-projection-profile/v1{lq-text-normalizer:2.0;fold:unicode}";

/// Infallible by construction: SHA-256 over the length-prefixed domain and
/// payload cannot fail.
fn compile_domain_digest(domain: &str, payload: &[u8]) -> [u8; 32] {
    let domain_length = u32::try_from(domain.len()).map_or(u32::MAX, |len| len);
    let mut hasher = Sha256::new();
    hasher.update(domain_length.to_be_bytes());
    hasher.update(domain.as_bytes());
    hasher.update(payload);
    hasher.finalize().into()
}

struct CompileCborWriter {
    bytes: Vec<u8>,
}

impl CompileCborWriter {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn write_array_header(&mut self, len: usize) {
        self.write_u64(u64::try_from(len).map_or(u64::MAX, |len| len));
    }

    fn write_u64(&mut self, value: u64) {
        if let Ok(byte) = u8::try_from(value) {
            if value > 23 {
                self.bytes.push(0x18);
            }
            self.bytes.push(byte);
        } else if let Ok(short) = u16::try_from(value) {
            self.bytes.push(0x19);
            self.bytes.extend_from_slice(&short.to_be_bytes());
        } else if let Ok(word) = u32::try_from(value) {
            self.bytes.push(0x1a);
            self.bytes.extend_from_slice(&word.to_be_bytes());
        } else {
            self.bytes.push(0x1b);
            self.bytes.extend_from_slice(&value.to_be_bytes());
        }
    }

    fn write_i64(&mut self, value: i64) {
        if value >= 0 {
            self.write_u64(u64::try_from(value).map_or(u64::MAX, |value| value));
        } else {
            let magnitude = i128::from(value).abs();
            self.write_negative(magnitude);
        }
    }

    fn write_negative(&mut self, magnitude: i128) {
        debug_assert!(magnitude >= 0);
        if let Ok(byte) = u8::try_from(magnitude) {
            if magnitude <= 23 {
                self.bytes.push(0x20 | byte);
            } else {
                self.bytes.push(0x38);
                self.bytes.push(byte);
            }
        } else if let Ok(short) = u16::try_from(magnitude) {
            self.bytes.push(0x39);
            self.bytes.extend_from_slice(&short.to_be_bytes());
        } else if let Ok(word) = u32::try_from(magnitude) {
            self.bytes.push(0x3a);
            self.bytes.extend_from_slice(&word.to_be_bytes());
        } else {
            self.bytes.push(0x3b);
            let long = u64::try_from(magnitude).map_or(u64::MAX, |long| long);
            self.bytes.extend_from_slice(&long.to_be_bytes());
        }
    }

    fn write_text(&mut self, text: &str) {
        let utf8 = text.as_bytes();
        let len = u64::try_from(utf8.len()).map_or(u64::MAX, |len| len);
        if let Ok(byte) = u8::try_from(len) {
            if len <= 23 {
                self.bytes.push(0x60 | byte);
            } else {
                self.bytes.push(0x78);
                self.bytes.push(byte);
            }
        } else if let Ok(short) = u16::try_from(len) {
            self.bytes.push(0x79);
            self.bytes.extend_from_slice(&short.to_be_bytes());
        } else if let Ok(word) = u32::try_from(len) {
            self.bytes.push(0x7a);
            self.bytes.extend_from_slice(&word.to_be_bytes());
        } else {
            self.bytes.push(0x7b);
            self.bytes.extend_from_slice(&len.to_be_bytes());
        }
        self.bytes.extend_from_slice(utf8);
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Infallible by construction: text lengths are bounded by the source strings.
fn write_node_ref(writer: &mut CompileCborWriter, node_ref: &RepoMapNodeRef) {
    match node_ref {
        RepoMapNodeRef::File(id) => {
            writer.write_array_header(2);
            writer.write_text("File");
            writer.write_text(id.as_str());
        }
        RepoMapNodeRef::Module(id) => {
            writer.write_array_header(2);
            writer.write_text("Module");
            writer.write_text(id.as_str());
        }
        RepoMapNodeRef::Symbol(id) => {
            writer.write_array_header(2);
            writer.write_text("Symbol");
            writer.write_text(id.as_str());
        }
        RepoMapNodeRef::Chunk(id) => {
            writer.write_array_header(2);
            writer.write_text("Chunk");
            writer.write_text(id.as_str());
        }
    }
}
