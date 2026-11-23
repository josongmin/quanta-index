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

use super::{
    CompiledRepoMapCandidateV1, CompiledRepoMapEdgeV1, CompiledRepoMapGraphV1,
    CompiledRepoMapNodeV1, CompiledRepoMapProjectionEntryV1, RepoMapCandidateCommitmentsV1,
    RepoMapChunkNode, RepoMapCompileRefusalCodeV1, RepoMapCompileRefusalV1, RepoMapCompileStageV1,
    RepoMapCompilerBudgetV1, RepoMapDocType, RepoMapEdge, RepoMapEdgeKind, RepoMapFileNode,
    RepoMapNode, RepoMapNodeRef, RepoMapResourceReceiptV1, RepoMapSourceBundle, RepoMapSymbolNode,
    RepoRelativePath,
};

const COMPILED_GRAPH_DOMAIN: &str = "quanta-index/repomap-compiled-graph/v1";
const COMPILED_SCHEMA_DOMAIN: &str = "quanta-index/repomap-compiled-schema/v1";
const PROJECTION_PROFILE_DOMAIN: &str = "quanta-index/repomap-projection-profile/v1";

/// Whole-bundle validated compiler. Deterministic: the compiled commitment is
/// independent of input node/edge order because every table is canonically
/// sorted before encoding.
pub struct RepoMapGraphCompiler {
    budget: RepoMapCompilerBudgetV1,
}

struct CompiledPartsV1 {
    graph: CompiledRepoMapGraphV1,
    projection: Vec<CompiledRepoMapProjectionEntryV1>,
    commitments: RepoMapCandidateCommitmentsV1,
    resource_receipt: RepoMapResourceReceiptV1,
    native_rank_valid: bool,
}

#[derive(Clone, Debug, Default)]
struct ChunkStatsV1<'a> {
    token_total: u32,
    preview_fragments: Vec<&'a str>,
}

#[derive(Clone, Debug)]
struct FileEntryInput<'a> {
    subject: RepoMapNodeRef,
    owner_path: &'a RepoRelativePath,
    line_count: u32,
}

#[derive(Clone, Debug)]
struct SymbolEntryInput<'a> {
    subject: RepoMapNodeRef,
    owner_path: &'a RepoRelativePath,
    local_name: &'a str,
    qualified_name: &'a str,
    symbol_kind: &'a str,
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
        let mut payload = Vec::new();
        let parts = self.compile_parts(bundle, None, |bytes| payload.extend_from_slice(bytes))?;
        Ok(CompiledRepoMapCandidateV1::from_parts(
            parts.graph,
            parts.projection,
            parts.commitments,
            payload,
            parts.resource_receipt,
        ))
    }

    /// Native graph publication retains graph, ranks, commitments, and exact
    /// budget receipts without allocating the legacy compiled CBOR payload.
    pub(crate) fn compile_native_graph_parts(
        &self,
        bundle: &RepoMapSourceBundle,
        selected_paths: Option<&BTreeSet<RepoRelativePath>>,
    ) -> Result<
        (
            CompiledRepoMapGraphV1,
            Vec<CompiledRepoMapProjectionEntryV1>,
            RepoMapCandidateCommitmentsV1,
            bool,
        ),
        RepoMapCompileRefusalV1,
    > {
        let parts = self.compile_parts(bundle, selected_paths, |_| {})?;
        Ok((
            parts.graph,
            parts.projection,
            parts.commitments,
            parts.native_rank_valid,
        ))
    }

    fn compile_parts(
        &self,
        bundle: &RepoMapSourceBundle,
        selected_paths: Option<&BTreeSet<RepoRelativePath>>,
        mut write_payload: impl FnMut(&[u8]),
    ) -> Result<CompiledPartsV1, RepoMapCompileRefusalV1> {
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
        let owner_symbols = identities
            .iter()
            .filter(|identity| matches!(identity, RepoMapNodeRef::Symbol(_)))
            .count();
        let owner_symbols = u64::try_from(owner_symbols).map_err(|_error| overflow())?;
        if owner_symbols > self.budget.max_owner_symbols {
            return Err(refusal(
                RepoMapCompileStageV1::Projection,
                RepoMapCompileRefusalCodeV1::OwnerSymbolLimitExceeded,
                Some(self.budget.max_owner_symbols),
                owner_symbols,
            ));
        }

        // Stage: Projection ---------------------------------------------------
        let degrees = graph_degrees(&canonical_edges);
        let (
            projection,
            preview_bytes,
            omitted_materialized_bytes,
            rank_row_count,
            native_rank_valid,
        ) = self.build_projection(bundle, &canonical_edges, &degrees, selected_paths)?;

        // Stage: Commitment ---------------------------------------------------
        let nodes = identities
            .into_iter()
            .map(|identity| {
                let (degree_in, degree_out) = degrees.get(&identity).copied().unwrap_or((0, 0));
                CompiledRepoMapNodeV1 {
                    identity,
                    degree_in,
                    degree_out,
                }
            })
            .collect();
        drop(degrees);
        let canonical_graph = CompiledRepoMapGraphV1 {
            nodes,
            edges: canonical_edges,
        };
        let mut materialized_bytes = Some(0_u64);
        let mut graph_digest = compile_domain_hasher(COMPILED_GRAPH_DOMAIN);
        write_compiled_payload(
            &mut CompileCborWriter(|bytes: &[u8]| {
                materialized_bytes = materialized_bytes.and_then(|value| {
                    u64::try_from(bytes.len())
                        .ok()
                        .and_then(|len| value.checked_add(len))
                });
                write_payload(bytes);
            }),
            &canonical_graph,
            &projection,
            rank_row_count,
            |bytes| graph_digest.update(bytes),
        );
        let materialized_bytes = materialized_bytes
            .and_then(|bytes| bytes.checked_add(omitted_materialized_bytes))
            .ok_or_else(overflow)?;
        check_cap(
            RepoMapCompileRefusalCodeV1::MaterializedByteLimitExceeded,
            self.budget.max_materialized_bytes,
            materialized_bytes,
        )?;
        let commitments = RepoMapCandidateCommitmentsV1 {
            producer_manifest,
            producer_authority,
            compiled_graph: graph_digest.finalize().into(),
            schema: compile_domain_digest(
                COMPILED_SCHEMA_DOMAIN,
                REPOMAP_COMPILE_SCHEMA_DESCRIPTOR.as_bytes(),
            ),
            projection_profile: compile_domain_digest(
                PROJECTION_PROFILE_DOMAIN,
                REPOMAP_COMPILE_PROJECTION_PROFILE_DESCRIPTOR.as_bytes(),
            ),
        };
        let resource_receipt = RepoMapResourceReceiptV1 {
            nodes: node_count,
            edges: edge_count,
            owner_symbols,
            preview_bytes,
            materialized_bytes,
            work_units,
            budget: self.budget,
        };
        debug_assert!(resource_receipt.observed());
        Ok(CompiledPartsV1 {
            graph: canonical_graph,
            projection,
            commitments,
            resource_receipt,
            native_rank_valid,
        })
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
        let mut edges = BTreeMap::new();
        for edge in &bundle.edges {
            let (kind, source, target) = edge_parts_v1(edge);
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
            if edges
                .insert((kind.as_code_str(), source, target), kind)
                .is_some()
            {
                return Err(refusal(
                    RepoMapCompileStageV1::GraphValidation,
                    RepoMapCompileRefusalCodeV1::DuplicateNode,
                    None,
                    edges_len(bundle),
                ));
            }
        }
        Ok(edges
            .into_iter()
            .map(|((_, source, target), kind)| CompiledRepoMapEdgeV1 {
                kind,
                source: source.clone(),
                target: target.clone(),
            })
            .collect())
    }

    fn build_projection(
        &self,
        bundle: &RepoMapSourceBundle,
        canonical_edges: &[CompiledRepoMapEdgeV1],
        degrees: &BTreeMap<&RepoMapNodeRef, (u32, u32)>,
        selected_paths: Option<&BTreeSet<RepoRelativePath>>,
    ) -> Result<
        (Vec<CompiledRepoMapProjectionEntryV1>, u64, u64, usize, bool),
        RepoMapCompileRefusalV1,
    > {
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
                    owner_path: repo_relative_path,
                    line_count: *line_count,
                }),
                RepoMapNode::Symbol(RepoMapSymbolNode {
                    symbol_id,
                    owner_path,
                    local_name,
                    qualified_name,
                    symbol_kind,
                }) => symbols.push(SymbolEntryInput {
                    subject: RepoMapNodeRef::Symbol(symbol_id.clone()),
                    owner_path,
                    local_name,
                    qualified_name,
                    symbol_kind: symbol_kind.as_str(),
                }),
                RepoMapNode::Module(_) | RepoMapNode::Chunk(_) => {}
            }
        }
        let chunk_stats = validated_chunk_stats(bundle, canonical_edges)?;
        let mut projection = Vec::new();
        let mut preview_bytes = 0_u64;
        let mut omitted_materialized_bytes = Some(0_u64);
        let mut native_rank_valid = true;
        let rank_row_count = files
            .len()
            .checked_add(symbols.len())
            .ok_or_else(overflow)?;
        let mut charge_preview = |bytes: usize| {
            let length = u64::try_from(bytes).map_err(|_error| overflow())?;
            preview_bytes = preview_bytes.checked_add(length).ok_or_else(overflow)?;
            check_cap(
                RepoMapCompileRefusalCodeV1::PreviewLimitExceeded,
                self.budget.max_preview_bytes,
                preview_bytes,
            )
        };
        for file in &files {
            let identity = node_ref_key(&file.subject);
            let prefix = [identity.as_str(), file.owner_path.as_str(), "file"];
            let chunks = chunk_stats.0.get(file.owner_path.as_str());
            let search_text_len = rank_search_text_len(&prefix, chunks)?;
            charge_preview(search_text_len)?;
            if selected_paths.is_some_and(|selected| !selected.contains(file.owner_path)) {
                native_rank_valid &=
                    native_rank_shape_valid(&file.subject, file.owner_path, search_text_len);
                let row_bytes = omitted_rank_row_bytes(
                    &file.subject,
                    RepoMapDocType::File,
                    file.owner_path,
                    search_text_len,
                )
                .ok();
                omitted_materialized_bytes = omitted_materialized_bytes
                    .and_then(|count| row_bytes.and_then(|bytes| count.checked_add(bytes)));
                continue;
            }
            let search_text = rank_search_text(&prefix, chunks, search_text_len);
            let degree = degrees.get(&file.subject).copied().unwrap_or((0, 0));
            let graph_degree = degree.0.saturating_add(degree.1);
            let final_score_millis = file_score_millis(file, &chunk_stats.0, graph_degree);
            projection.push(CompiledRepoMapProjectionEntryV1 {
                subject: file.subject.clone(),
                doc_type: RepoMapDocType::File,
                owner_path: file.owner_path.to_owned(),
                search_text,
                final_score_millis: i64::from(final_score_millis),
            });
        }
        for symbol in &symbols {
            let identity = node_ref_key(&symbol.subject);
            let prefix = [
                identity.as_str(),
                symbol.local_name,
                symbol.qualified_name,
                symbol.symbol_kind,
                symbol.owner_path.as_str(),
            ];
            let chunks = chunk_stats.1.get(&symbol.subject);
            let search_text_len = rank_search_text_len(&prefix, chunks)?;
            charge_preview(search_text_len)?;
            if selected_paths.is_some_and(|selected| !selected.contains(symbol.owner_path)) {
                native_rank_valid &=
                    native_rank_shape_valid(&symbol.subject, symbol.owner_path, search_text_len);
                let row_bytes = omitted_rank_row_bytes(
                    &symbol.subject,
                    RepoMapDocType::Symbol,
                    symbol.owner_path,
                    search_text_len,
                )
                .ok();
                omitted_materialized_bytes = omitted_materialized_bytes
                    .and_then(|count| row_bytes.and_then(|bytes| count.checked_add(bytes)));
                continue;
            }
            let search_text = rank_search_text(&prefix, chunks, search_text_len);
            let degree = degrees.get(&symbol.subject).copied().unwrap_or((0, 0));
            let graph_degree = degree.0.saturating_add(degree.1);
            let final_score_millis = symbol_score_millis(symbol, &chunk_stats.1, graph_degree);
            projection.push(CompiledRepoMapProjectionEntryV1 {
                subject: symbol.subject.clone(),
                doc_type: RepoMapDocType::Symbol,
                owner_path: symbol.owner_path.to_owned(),
                search_text,
                final_score_millis: i64::from(final_score_millis),
            });
        }
        projection.sort_by(|lhs, rhs| lhs.subject.cmp(&rhs.subject));
        Ok((
            projection,
            preview_bytes,
            omitted_materialized_bytes.ok_or_else(overflow)?,
            rank_row_count,
            native_rank_valid,
        ))
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

pub(crate) fn edge_parts_v1(
    edge: &RepoMapEdge,
) -> (RepoMapEdgeKind, &RepoMapNodeRef, &RepoMapNodeRef) {
    match edge {
        RepoMapEdge::Contains(edge) => {
            (RepoMapEdgeKind::Contains, &edge.container, &edge.contained)
        }
        RepoMapEdge::Call(edge) => (RepoMapEdgeKind::Call, &edge.caller, &edge.callee),
        RepoMapEdge::Import(edge) => (RepoMapEdgeKind::Import, &edge.importer, &edge.imported),
        RepoMapEdge::OwnsChunk(edge) => (RepoMapEdgeKind::OwnsChunk, &edge.owner, &edge.chunk),
        RepoMapEdge::DependsOn(edge) => (
            RepoMapEdgeKind::DependsOn,
            &edge.dependent,
            &edge.dependency,
        ),
    }
}

pub(crate) fn node_identity(node: &RepoMapNode) -> RepoMapNodeRef {
    match node {
        RepoMapNode::File(inner) => RepoMapNodeRef::File(inner.file_id.clone()),
        RepoMapNode::Module(inner) => RepoMapNodeRef::Module(inner.module_id.clone()),
        RepoMapNode::Symbol(inner) => RepoMapNodeRef::Symbol(inner.symbol_id.clone()),
        RepoMapNode::Chunk(inner) => RepoMapNodeRef::Chunk(inner.chunk_id.clone()),
    }
}

#[doc(hidden)]
pub fn node_ref_parts(node_ref: &RepoMapNodeRef) -> (String, u8) {
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

#[doc(hidden)]
pub fn producer_digest(text: &str) -> Result<[u8; 32], RepoMapCompileRefusalV1> {
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
type ChunkStatsTables<'a> = (
    BTreeMap<&'a str, ChunkStatsV1<'a>>,
    BTreeMap<&'a RepoMapNodeRef, ChunkStatsV1<'a>>,
);

fn validated_chunk_stats<'a>(
    bundle: &'a RepoMapSourceBundle,
    canonical_edges: &'a [CompiledRepoMapEdgeV1],
) -> Result<ChunkStatsTables<'a>, RepoMapCompileRefusalV1> {
    let mut by_owner = BTreeMap::new();
    let mut by_subject = BTreeMap::new();
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
            by_owner.entry(chunk.owner_path.as_str()).or_default(),
            chunk,
        )?;
    }
    for edge in canonical_edges {
        if edge.kind != RepoMapEdgeKind::OwnsChunk {
            continue;
        }
        let RepoMapNodeRef::Chunk(chunk_id) = &edge.target else {
            continue;
        };
        if let Some(chunk) = chunks.get(chunk_id.as_str()) {
            accumulate_chunk(by_subject.entry(&edge.source).or_default(), chunk)?;
        }
    }
    Ok((by_owner, by_subject))
}

fn accumulate_chunk<'a>(
    stats: &mut ChunkStatsV1<'a>,
    chunk: &'a RepoMapChunkNode,
) -> Result<(), RepoMapCompileRefusalV1> {
    stats.token_total = stats
        .token_total
        .checked_add(chunk.token_count)
        .ok_or_else(overflow)?;
    let trimmed = chunk.preview_text.trim();
    if !trimmed.is_empty() {
        stats.preview_fragments.push(trimmed);
    }
    Ok(())
}

fn node_ref_key(node_ref: &RepoMapNodeRef) -> String {
    let (raw, discriminant) = node_ref_parts(node_ref);
    format!("{discriminant}:{raw}")
}

fn native_rank_shape_valid(
    subject: &RepoMapNodeRef,
    owner_path: &RepoRelativePath,
    search_text_len: usize,
) -> bool {
    use crate::native_protocol::{
        MAX_REPOMAP_RANK_PATH_BYTES, MAX_REPOMAP_RANK_SEARCH_TEXT_BYTES,
        MAX_REPOMAP_RANK_SUBJECT_BYTES,
    };
    let raw = match subject {
        RepoMapNodeRef::File(id) => id.as_str(),
        RepoMapNodeRef::Symbol(id) => id.as_str(),
        RepoMapNodeRef::Module(_) | RepoMapNodeRef::Chunk(_) => return false,
    };
    !raw.is_empty()
        && raw.len() <= MAX_REPOMAP_RANK_SUBJECT_BYTES
        && !owner_path.as_str().is_empty()
        && owner_path.as_str().len() <= MAX_REPOMAP_RANK_PATH_BYTES
        && search_text_len <= MAX_REPOMAP_RANK_SEARCH_TEXT_BYTES
}

fn rank_search_text_len<'a>(
    prefix: &[&'a str],
    chunks: Option<&ChunkStatsV1<'a>>,
) -> Result<usize, RepoMapCompileRefusalV1> {
    let previews = chunks.map_or(&[][..], |stats| stats.preview_fragments.as_slice());
    let parts = prefix
        .iter()
        .chain(previews)
        .copied()
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let mut length = 0usize;
    for part in parts.clone() {
        let separator = if length == 0 { 0 } else { 1 };
        length = length
            .checked_add(separator)
            .and_then(|length| length.checked_add(part.len()))
            .ok_or_else(overflow)?;
    }
    Ok(length)
}

fn rank_search_text<'a>(
    prefix: &[&'a str],
    chunks: Option<&ChunkStatsV1<'a>>,
    length: usize,
) -> String {
    let previews = chunks.map_or(&[][..], |stats| stats.preview_fragments.as_slice());
    let mut output = String::with_capacity(length);
    for part in prefix
        .iter()
        .chain(previews)
        .copied()
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(part);
    }
    output
}

/// A file score is at least floor((280*4+240*3)/7)=262, and a symbol
/// score at least floor((420*4+320*3)/7)=377. Both are capped at 1000.
/// Canonical unsigned CBOR therefore uses 0x19 plus two bytes for every row.
const RANK_SCORE_CBOR_BYTES: u64 = 3;

fn omitted_rank_row_bytes(
    subject: &RepoMapNodeRef,
    doc_type: RepoMapDocType,
    owner_path: &RepoRelativePath,
    search_text_len: usize,
) -> Result<u64, RepoMapCompileRefusalV1> {
    let mut measured = Some(0_u64);
    {
        let mut writer = CompileCborWriter(|bytes: &[u8]| {
            measured = measured.and_then(|count| {
                u64::try_from(bytes.len())
                    .ok()
                    .and_then(|len| count.checked_add(len))
            });
        });
        writer.write_array_header(5);
        write_node_ref(&mut writer, subject);
        writer.write_text(doc_type.as_code_str());
        writer.write_text(owner_path.as_str());
        writer.write_text_header_len(search_text_len);
    }
    measured
        .and_then(|count| {
            u64::try_from(search_text_len)
                .ok()
                .and_then(|len| count.checked_add(len))
        })
        .and_then(|count| count.checked_add(RANK_SCORE_CBOR_BYTES))
        .ok_or_else(overflow)
}

fn file_score_millis(
    file: &FileEntryInput<'_>,
    owner_chunks: &BTreeMap<&str, ChunkStatsV1<'_>>,
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
    symbol: &SymbolEntryInput<'_>,
    subject_chunks: &BTreeMap<&RepoMapNodeRef, ChunkStatsV1<'_>>,
    graph_degree: u32,
) -> u32 {
    let chunk_total = subject_chunks
        .get(&symbol.subject)
        .map_or(0, |stats| stats.token_total);
    let importance = (420_u32).saturating_add(graph_degree.saturating_mul(35));
    let utility = (320_u32).saturating_add(chunk_total.div_euclid(4).min(220));
    let final_millis = importance
        .saturating_mul(4)
        .saturating_add(utility.saturating_mul(3))
        .div_euclid(7);
    final_millis.min(1_000)
}

fn graph_degrees(edges: &[CompiledRepoMapEdgeV1]) -> BTreeMap<&RepoMapNodeRef, (u32, u32)> {
    let mut degrees = BTreeMap::<&RepoMapNodeRef, (u32, u32)>::new();
    for edge in edges {
        let source = degrees.entry(&edge.source).or_default();
        source.0 = source.0.saturating_add(1);
        let target = degrees.entry(&edge.target).or_default();
        target.1 = target.1.saturating_add(1);
    }
    degrees
}

fn write_graph_table(
    writer: &mut CompileCborWriter<impl FnMut(&[u8])>,
    graph: &CompiledRepoMapGraphV1,
) {
    writer.write_array_header(graph.nodes.len());
    for node in &graph.nodes {
        writer.write_array_header(3);
        write_node_ref(writer, &node.identity);
        writer.write_u64(u64::from(node.degree_in));
        writer.write_u64(u64::from(node.degree_out));
    }
    writer.write_array_header(graph.edges.len());
    for edge in &graph.edges {
        writer.write_array_header(3);
        writer.write_text(edge.kind.as_code_str());
        write_node_ref(writer, &edge.source);
        write_node_ref(writer, &edge.target);
    }
}

#[doc(hidden)]
pub fn encode_compiled_payload(
    graph: &CompiledRepoMapGraphV1,
    projection: &[CompiledRepoMapProjectionEntryV1],
) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_compiled_payload(
        &mut CompileCborWriter(|slice: &[u8]| bytes.extend_from_slice(slice)),
        graph,
        projection,
        projection.len(),
        |_| {},
    );
    bytes
}

fn write_compiled_payload(
    writer: &mut CompileCborWriter<impl FnMut(&[u8])>,
    graph: &CompiledRepoMapGraphV1,
    projection: &[CompiledRepoMapProjectionEntryV1],
    projection_count: usize,
    mut graph_bytes: impl FnMut(&[u8]),
) {
    writer.write_array_header(2);
    write_graph_table(
        &mut CompileCborWriter(|bytes: &[u8]| {
            writer.extend_from_slice(bytes);
            graph_bytes(bytes);
        }),
        graph,
    );
    writer.write_array_header(projection_count);
    for entry in projection {
        writer.write_array_header(5);
        write_node_ref(writer, &entry.subject);
        writer.write_text(entry.doc_type.as_code_str());
        writer.write_text(entry.owner_path.as_str());
        writer.write_text(&entry.search_text);
        writer.write_i64(entry.final_score_millis);
    }
}

// ---------------------------------------------------------------------------
// Private canonical CBOR writer and digest helpers (compiler-owned schema).
// ---------------------------------------------------------------------------

pub(crate) const REPOMAP_COMPILE_SCHEMA_DESCRIPTOR: &str = "quanta-index/repomap-compiled-schema/v1{nodes:identity,degree_in,degree_out;edges:kind,source,target;projection:subject,doc_type,owner_path,search_text,final_score_millis}";

pub(crate) const REPOMAP_COMPILE_PROJECTION_PROFILE_DESCRIPTOR: &str =
    "quanta-index/repomap-projection-profile/v1{lq-text-normalizer:2.0;fold:unicode}";

/// Infallible by construction: SHA-256 over the length-prefixed domain and
/// payload cannot fail.
fn compile_domain_digest(domain: &str, payload: &[u8]) -> [u8; 32] {
    let mut hasher = compile_domain_hasher(domain);
    hasher.update(payload);
    hasher.finalize().into()
}

fn compile_domain_hasher(domain: &str) -> Sha256 {
    let domain_length = u32::try_from(domain.len()).map_or(u32::MAX, |len| len);
    let mut hasher = Sha256::new();
    hasher.update(domain_length.to_be_bytes());
    hasher.update(domain.as_bytes());
    hasher
}

struct CompileCborWriter<S>(S);

impl<S: FnMut(&[u8])> CompileCborWriter<S> {
    fn push(&mut self, byte: u8) {
        self.extend_from_slice(&[byte]);
    }

    fn extend_from_slice(&mut self, slice: &[u8]) {
        (self.0)(slice);
    }

    fn write_array_header(&mut self, len: usize) {
        self.write_u64(u64::try_from(len).map_or(u64::MAX, |len| len));
    }

    fn write_u64(&mut self, value: u64) {
        if let Ok(byte) = u8::try_from(value) {
            if value > 23 {
                self.push(0x18);
            }
            self.push(byte);
        } else if let Ok(short) = u16::try_from(value) {
            self.push(0x19);
            self.extend_from_slice(&short.to_be_bytes());
        } else if let Ok(word) = u32::try_from(value) {
            self.push(0x1a);
            self.extend_from_slice(&word.to_be_bytes());
        } else {
            self.push(0x1b);
            self.extend_from_slice(&value.to_be_bytes());
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
                self.push(0x20 | byte);
            } else {
                self.push(0x38);
                self.push(byte);
            }
        } else if let Ok(short) = u16::try_from(magnitude) {
            self.push(0x39);
            self.extend_from_slice(&short.to_be_bytes());
        } else if let Ok(word) = u32::try_from(magnitude) {
            self.push(0x3a);
            self.extend_from_slice(&word.to_be_bytes());
        } else {
            self.push(0x3b);
            let long = u64::try_from(magnitude).map_or(u64::MAX, |long| long);
            self.extend_from_slice(&long.to_be_bytes());
        }
    }

    fn write_text(&mut self, text: &str) {
        let utf8 = text.as_bytes();
        self.write_text_header_len(utf8.len());
        self.extend_from_slice(utf8);
    }

    fn write_text_header_len(&mut self, len: usize) {
        let len = u64::try_from(len).map_or(u64::MAX, |len| len);
        if let Ok(byte) = u8::try_from(len) {
            if len <= 23 {
                self.push(0x60 | byte);
            } else {
                self.push(0x78);
                self.push(byte);
            }
        } else if let Ok(short) = u16::try_from(len) {
            self.push(0x79);
            self.extend_from_slice(&short.to_be_bytes());
        } else if let Ok(word) = u32::try_from(len) {
            self.push(0x7a);
            self.extend_from_slice(&word.to_be_bytes());
        } else {
            self.push(0x7b);
            self.extend_from_slice(&len.to_be_bytes());
        }
    }
}

/// Infallible by construction: text lengths are bounded by the source strings.
fn write_node_ref(writer: &mut CompileCborWriter<impl FnMut(&[u8])>, node_ref: &RepoMapNodeRef) {
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
