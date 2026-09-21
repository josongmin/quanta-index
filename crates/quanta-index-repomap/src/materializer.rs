use std::collections::BTreeMap;

use quanta_index_contract::{
    RepoMapChunkExactness, RepoMapChunkNode, RepoMapDocType, RepoMapEdge, RepoMapFileNode,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode, RepoMapNodeRef,
    RepoMapSnapshotMeta, RepoMapSourceBundle, RepoMapSymbolNode,
    canonical_repo_map_source_bundle_digest_v1,
};

use crate::model::{RepoMapEntry, RepoMapSnapshot};

pub struct RepoMapMaterializer;

#[derive(Clone, Copy, Debug, Default)]
struct GraphStatsV1 {
    incoming: u32,
    outgoing: u32,
}

#[derive(Clone, Debug, Default)]
struct ChunkStatsV1 {
    token_total: u32,
    preview_fragments: Vec<String>,
    exactness_markers: Vec<RepoMapChunkExactness>,
}

#[derive(Clone, Debug)]
struct FileEntryInput {
    subject_identity: String,
    owner_path: String,
    line_count: u32,
}

#[derive(Clone, Debug)]
struct SymbolEntryInput {
    subject_identity: String,
    owner_path: String,
    local_name: String,
    qualified_name: String,
    symbol_kind: String,
}

impl RepoMapMaterializer {
    #[must_use]
    pub fn materialize(bundle: &RepoMapSourceBundle) -> Result<RepoMapSnapshot, String> {
        let snapshot_meta = RepoMapSnapshotMeta {
            snapshot_id: bundle.snapshot_id.clone(),
            projection_version: bundle.projection_version,
            authority_digest: bundle.authority_digest.clone(),
            item_index_availability: bundle.graph_coverage.item_index_availability,
            graph_coverage_class: bundle.graph_coverage.graph_coverage_class,
            exactness_summary: bundle.exactness_summary,
        };
        let projection_status = projection_status(bundle);
        let call_stats = call_graph_stats(&bundle.edges);
        let import_stats = import_graph_stats(&bundle.edges);
        let file_nodes = file_entry_inputs(bundle);
        let symbol_nodes = symbol_entry_inputs(bundle);
        let symbols_by_owner_path = group_symbols_by_owner_path(&symbol_nodes);
        let chunk_nodes = chunk_nodes_by_id(bundle);
        let (subject_chunks, owner_chunks) = chunk_stats(bundle, &chunk_nodes);
        let empty_graph_stats = GraphStatsV1::default();
        let empty_chunk_stats = ChunkStatsV1::default();

        let mut entries = Vec::new();
        for file in &file_nodes {
            let file_symbols = symbols_by_owner_path
                .get(file.owner_path.as_str())
                .cloned()
                .unwrap_or_default();
            let file_call_stats = call_stats
                .get(file.subject_identity.as_str())
                .unwrap_or(&empty_graph_stats);
            let file_import_stats = import_stats
                .get(file.subject_identity.as_str())
                .unwrap_or(&empty_graph_stats);
            let file_chunk_stats = owner_chunks
                .get(file.owner_path.as_str())
                .unwrap_or(&empty_chunk_stats);
            entries.push(build_file_entry(
                bundle,
                file,
                &file_symbols,
                &projection_status,
                file_call_stats,
                file_import_stats,
                file_chunk_stats,
            ));
        }
        for symbol in &symbol_nodes {
            let symbol_call_stats = call_stats
                .get(symbol.subject_identity.as_str())
                .unwrap_or(&empty_graph_stats);
            let symbol_import_stats = import_stats
                .get(symbol.subject_identity.as_str())
                .unwrap_or(&empty_graph_stats);
            let subject_chunk = subject_chunks
                .get(symbol.subject_identity.as_str())
                .or_else(|| owner_chunks.get(symbol.owner_path.as_str()))
                .unwrap_or(&empty_chunk_stats);
            entries.push(build_symbol_entry(
                bundle,
                symbol,
                &projection_status,
                symbol_call_stats,
                symbol_import_stats,
                subject_chunk,
            ));
        }

        entries.sort_by(|lhs, rhs| {
            rhs.final_score_millis
                .cmp(&lhs.final_score_millis)
                .then(lhs.subject_identity.cmp(&rhs.subject_identity))
        });

        Ok(RepoMapSnapshot {
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
            manifest_generation: bundle.manifest_generation,
            manifest_digest: Some(bundle.manifest_digest.clone()),
            source_bundle_digest: Some(canonical_repo_map_source_bundle_digest_v1(bundle)?),
            snapshot_meta,
            entries,
        })
    }
}

fn file_entry_inputs(bundle: &RepoMapSourceBundle) -> Vec<FileEntryInput> {
    bundle
        .nodes
        .iter()
        .filter_map(|node| match node {
            RepoMapNode::File(RepoMapFileNode {
                file_id,
                repo_relative_path,
                line_count,
            }) => Some(FileEntryInput {
                subject_identity: file_id.as_str().to_string(),
                owner_path: repo_relative_path.as_str().to_string(),
                line_count: *line_count,
            }),
            RepoMapNode::Module(_) | RepoMapNode::Symbol(_) | RepoMapNode::Chunk(_) => None,
        })
        .collect()
}

fn symbol_entry_inputs(bundle: &RepoMapSourceBundle) -> Vec<SymbolEntryInput> {
    bundle
        .nodes
        .iter()
        .filter_map(|node| match node {
            RepoMapNode::Symbol(RepoMapSymbolNode {
                symbol_id,
                owner_path,
                local_name,
                qualified_name,
                symbol_kind,
            }) => Some(SymbolEntryInput {
                subject_identity: symbol_id.as_str().to_string(),
                owner_path: owner_path.as_str().to_string(),
                local_name: local_name.clone(),
                qualified_name: qualified_name.clone(),
                symbol_kind: symbol_kind.as_str().to_string(),
            }),
            RepoMapNode::File(_) | RepoMapNode::Module(_) | RepoMapNode::Chunk(_) => None,
        })
        .collect()
}

fn group_symbols_by_owner_path(
    symbols: &[SymbolEntryInput],
) -> BTreeMap<&str, Vec<SymbolEntryInput>> {
    let mut grouped = BTreeMap::<&str, Vec<SymbolEntryInput>>::new();
    for symbol in symbols {
        grouped
            .entry(symbol.owner_path.as_str())
            .or_default()
            .push(symbol.clone());
    }
    grouped
}

fn chunk_nodes_by_id(bundle: &RepoMapSourceBundle) -> BTreeMap<String, RepoMapChunkNode> {
    bundle
        .nodes
        .iter()
        .filter_map(|node| match node {
            RepoMapNode::Chunk(chunk) => Some((chunk.chunk_id.as_str().to_string(), chunk.clone())),
            RepoMapNode::File(_) | RepoMapNode::Module(_) | RepoMapNode::Symbol(_) => None,
        })
        .collect()
}

fn build_file_entry(
    bundle: &RepoMapSourceBundle,
    file: &FileEntryInput,
    file_symbols: &[SymbolEntryInput],
    projection_status: &str,
    call_stats: &GraphStatsV1,
    import_stats: &GraphStatsV1,
    chunk_stats: &ChunkStatsV1,
) -> RepoMapEntry {
    let symbol_count = saturating_u32_from_usize(file_symbols.len());
    let graph_degree = total_degree(call_stats).saturating_add(total_degree(import_stats));
    let line_bonus = file.line_count.div_euclid(8).min(120);
    let importance = clamp_score(
        280_u32
            .saturating_add(symbol_count.saturating_mul(55))
            .saturating_add(graph_degree.saturating_mul(28))
            .saturating_add(line_bonus),
    );
    let utility = clamp_score(
        240_u32
            .saturating_add(chunk_stats.token_total.div_euclid(3).min(260))
            .saturating_add(call_stats.outgoing.saturating_mul(22))
            .saturating_add(import_stats.outgoing.saturating_mul(18)),
    );
    let freshness = freshness_score(bundle);
    let evidence_priority = clamp_score(
        520_u32
            .saturating_add(exactness_signal(bundle, chunk_stats))
            .saturating_add(if projection_status == "Complete" {
                120
            } else {
                20
            }),
    );
    let final_score_millis = weighted_final_score(
        importance,
        utility,
        freshness,
        evidence_priority,
        graph_degree,
        symbol_count,
    );
    let search_text = build_search_text(vec![
        file.subject_identity.clone(),
        file.owner_path.clone(),
        "file".to_string(),
        file_symbols
            .iter()
            .map(|record| record.local_name.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        chunk_stats.preview_fragments.join(" "),
        chunk_stats
            .exactness_markers
            .iter()
            .map(|marker| marker.as_code_str())
            .collect::<Vec<_>>()
            .join(" "),
        bundle
            .graph_coverage
            .item_index_availability
            .as_code_str()
            .to_string(),
        bundle
            .graph_coverage
            .graph_coverage_class
            .as_code_str()
            .to_string(),
    ]);
    RepoMapEntry {
        subject_identity: file.subject_identity.clone(),
        subject_doc_type: RepoMapDocType::File,
        subject_kind: "file".to_string(),
        owner_path: file.owner_path.clone(),
        score: score_from_millis(final_score_millis),
        final_score_millis,
        importance_score_millis: importance,
        utility_score_millis: utility,
        freshness_score_millis: freshness,
        evidence_priority_millis: evidence_priority,
        token_budget_hint: token_hint(
            chunk_stats.token_total.max(file.line_count.div_euclid(2)),
            symbol_count,
        ),
        contributing_signals: BTreeMap::from([
            ("symbol_count".to_string(), i64::from(symbol_count)),
            ("line_count".to_string(), i64::from(file.line_count)),
            (
                "call_incoming_edges".to_string(),
                i64::from(call_stats.incoming),
            ),
            (
                "call_outgoing_edges".to_string(),
                i64::from(call_stats.outgoing),
            ),
            (
                "import_incoming_edges".to_string(),
                i64::from(import_stats.incoming),
            ),
            (
                "import_outgoing_edges".to_string(),
                i64::from(import_stats.outgoing),
            ),
            (
                "chunk_token_total".to_string(),
                i64::from(chunk_stats.token_total),
            ),
        ]),
        projection_evidence_kind: "AuthorityBundle".to_string(),
        projection_authority_artifact_id: authority_artifact_id(
            &bundle.snapshot_id,
            &file.subject_identity,
        ),
        projection_authority_digest: bundle.authority_digest.clone(),
        projection_status: projection_status.to_string(),
        redaction_state: bundle.redaction_state,
        search_text,
        source_symbol_count: symbol_count,
        source_chunk_token_total: chunk_stats.token_total,
        source_call_incoming_edges: call_stats.incoming,
        source_call_outgoing_edges: call_stats.outgoing,
        source_import_incoming_edges: import_stats.incoming,
        source_import_outgoing_edges: import_stats.outgoing,
    }
}

fn build_symbol_entry(
    bundle: &RepoMapSourceBundle,
    symbol: &SymbolEntryInput,
    projection_status: &str,
    call_stats: &GraphStatsV1,
    import_stats: &GraphStatsV1,
    chunk_stats: &ChunkStatsV1,
) -> RepoMapEntry {
    let graph_degree = total_degree(call_stats).saturating_add(total_degree(import_stats));
    let importance = clamp_score(
        420_u32
            .saturating_add(graph_degree.saturating_mul(35))
            .saturating_add(chunk_stats.token_total.div_euclid(5).min(160)),
    );
    let utility = clamp_score(
        320_u32
            .saturating_add(call_stats.outgoing.saturating_mul(32))
            .saturating_add(import_stats.outgoing.saturating_mul(18))
            .saturating_add(chunk_stats.token_total.div_euclid(4).min(220)),
    );
    let freshness = freshness_score(bundle);
    let evidence_priority = clamp_score(
        640_u32
            .saturating_add(exactness_signal(bundle, chunk_stats))
            .saturating_add(if projection_status == "Complete" {
                80
            } else {
                0
            }),
    );
    let final_score_millis = weighted_final_score(
        importance,
        utility,
        freshness,
        evidence_priority,
        graph_degree,
        1,
    );
    let search_text = build_search_text(vec![
        symbol.subject_identity.clone(),
        symbol.local_name.clone(),
        symbol.qualified_name.clone(),
        symbol.symbol_kind.clone(),
        symbol.owner_path.clone(),
        chunk_stats.preview_fragments.join(" "),
        chunk_stats
            .exactness_markers
            .iter()
            .map(|marker| marker.as_code_str())
            .collect::<Vec<_>>()
            .join(" "),
        bundle.exactness_summary.as_code_str().to_string(),
    ]);
    RepoMapEntry {
        subject_identity: symbol.subject_identity.clone(),
        subject_doc_type: RepoMapDocType::Symbol,
        subject_kind: symbol.symbol_kind.clone(),
        owner_path: symbol.owner_path.clone(),
        score: score_from_millis(final_score_millis),
        final_score_millis,
        importance_score_millis: importance,
        utility_score_millis: utility,
        freshness_score_millis: freshness,
        evidence_priority_millis: evidence_priority,
        token_budget_hint: token_hint(chunk_stats.token_total.max(48), 1),
        contributing_signals: BTreeMap::from([
            ("symbol_count".to_string(), 1_i64),
            (
                "call_incoming_edges".to_string(),
                i64::from(call_stats.incoming),
            ),
            (
                "call_outgoing_edges".to_string(),
                i64::from(call_stats.outgoing),
            ),
            (
                "import_incoming_edges".to_string(),
                i64::from(import_stats.incoming),
            ),
            (
                "import_outgoing_edges".to_string(),
                i64::from(import_stats.outgoing),
            ),
            (
                "chunk_token_total".to_string(),
                i64::from(chunk_stats.token_total),
            ),
        ]),
        projection_evidence_kind: "AuthorityBundle".to_string(),
        projection_authority_artifact_id: authority_artifact_id(
            &bundle.snapshot_id,
            &symbol.subject_identity,
        ),
        projection_authority_digest: bundle.authority_digest.clone(),
        projection_status: projection_status.to_string(),
        redaction_state: bundle.redaction_state,
        search_text,
        source_symbol_count: 1,
        source_chunk_token_total: chunk_stats.token_total,
        source_call_incoming_edges: call_stats.incoming,
        source_call_outgoing_edges: call_stats.outgoing,
        source_import_incoming_edges: import_stats.incoming,
        source_import_outgoing_edges: import_stats.outgoing,
    }
}

fn call_graph_stats(edges: &[RepoMapEdge]) -> BTreeMap<String, GraphStatsV1> {
    let mut stats = BTreeMap::<String, GraphStatsV1>::new();
    for edge in edges {
        let RepoMapEdge::Call(call) = edge else {
            continue;
        };
        let from_identity = node_ref_identity(&call.caller);
        let to_identity = node_ref_identity(&call.callee);
        let from = stats.entry(from_identity).or_default();
        from.outgoing = from.outgoing.saturating_add(1);
        let to = stats.entry(to_identity).or_default();
        to.incoming = to.incoming.saturating_add(1);
    }
    stats
}

fn import_graph_stats(edges: &[RepoMapEdge]) -> BTreeMap<String, GraphStatsV1> {
    let mut stats = BTreeMap::<String, GraphStatsV1>::new();
    for edge in edges {
        let RepoMapEdge::Import(import) = edge else {
            continue;
        };
        let from_identity = node_ref_identity(&import.importer);
        let to_identity = node_ref_identity(&import.imported);
        let from = stats.entry(from_identity).or_default();
        from.outgoing = from.outgoing.saturating_add(1);
        let to = stats.entry(to_identity).or_default();
        to.incoming = to.incoming.saturating_add(1);
    }
    stats
}

fn chunk_stats(
    bundle: &RepoMapSourceBundle,
    chunk_nodes: &BTreeMap<String, RepoMapChunkNode>,
) -> (
    BTreeMap<String, ChunkStatsV1>,
    BTreeMap<String, ChunkStatsV1>,
) {
    let mut by_subject = BTreeMap::<String, ChunkStatsV1>::new();
    let mut by_owner = BTreeMap::<String, ChunkStatsV1>::new();
    for chunk in chunk_nodes.values() {
        accumulate_chunk(
            by_owner
                .entry(chunk.owner_path.as_str().to_string())
                .or_default(),
            chunk,
        );
    }
    for edge in &bundle.edges {
        let RepoMapEdge::OwnsChunk(owns) = edge else {
            continue;
        };
        let RepoMapNodeRef::Chunk(chunk_id) = &owns.chunk else {
            continue;
        };
        let Some(chunk) = chunk_nodes.get(chunk_id.as_str()) else {
            continue;
        };
        accumulate_chunk(
            by_subject
                .entry(node_ref_identity(&owns.owner))
                .or_default(),
            chunk,
        );
    }
    (by_subject, by_owner)
}

fn accumulate_chunk(stats: &mut ChunkStatsV1, chunk: &RepoMapChunkNode) {
    stats.token_total = stats.token_total.saturating_add(chunk.token_count);
    if !chunk.preview_text.trim().is_empty() {
        stats
            .preview_fragments
            .push(chunk.preview_text.trim().to_string());
    }
    stats.exactness_markers.push(chunk.exactness);
}

fn node_ref_identity(node_ref: &RepoMapNodeRef) -> String {
    match node_ref {
        RepoMapNodeRef::File(file_id) => file_id.as_str().to_string(),
        RepoMapNodeRef::Module(module_id) => module_id.as_str().to_string(),
        RepoMapNodeRef::Symbol(symbol_id) => symbol_id.as_str().to_string(),
        RepoMapNodeRef::Chunk(chunk_id) => chunk_id.as_str().to_string(),
    }
}

fn total_degree(stats: &GraphStatsV1) -> u32 {
    stats.incoming.saturating_add(stats.outgoing)
}

fn freshness_score(bundle: &RepoMapSourceBundle) -> u32 {
    if matches!(
        bundle.graph_coverage.item_index_availability,
        RepoMapItemIndexAvailability::Available | RepoMapItemIndexAvailability::Full
    ) && matches!(
        bundle.graph_coverage.graph_coverage_class,
        RepoMapGraphCoverageClass::Complete | RepoMapGraphCoverageClass::Full
    ) {
        return 860;
    }
    if matches!(
        bundle.graph_coverage.item_index_availability,
        RepoMapItemIndexAvailability::Partial
    ) || matches!(
        bundle.graph_coverage.graph_coverage_class,
        RepoMapGraphCoverageClass::Partial
    ) {
        return 620;
    }
    500
}

fn projection_status(bundle: &RepoMapSourceBundle) -> String {
    if matches!(
        bundle.graph_coverage.item_index_availability,
        RepoMapItemIndexAvailability::Available | RepoMapItemIndexAvailability::Full
    ) && matches!(
        bundle.graph_coverage.graph_coverage_class,
        RepoMapGraphCoverageClass::Complete | RepoMapGraphCoverageClass::Full
    ) {
        return "Complete".to_string();
    }
    "Partial".to_string()
}

fn exactness_signal(bundle: &RepoMapSourceBundle, chunk_stats: &ChunkStatsV1) -> u32 {
    let mut score: u32 = if matches!(
        bundle.exactness_summary,
        quanta_index_contract::RepoMapExactnessSummary::Exact
    ) {
        140
    } else {
        40
    };
    if chunk_stats
        .exactness_markers
        .iter()
        .any(|marker| matches!(marker, RepoMapChunkExactness::Exact))
    {
        score = score.saturating_add(80);
    }
    score.min(260)
}

fn token_hint(base_tokens: u32, symbol_count: u32) -> u32 {
    base_tokens
        .saturating_add(symbol_count.saturating_mul(12))
        .clamp(48, 256)
}

fn weighted_final_score(
    importance: u32,
    utility: u32,
    freshness: u32,
    evidence_priority: u32,
    graph_degree: u32,
    symbol_count: u32,
) -> u32 {
    let weighted = importance
        .saturating_mul(4)
        .saturating_add(utility.saturating_mul(3))
        .saturating_add(freshness.saturating_mul(2))
        .saturating_add(evidence_priority);
    let normalized = weighted.div_euclid(10);
    clamp_score(
        normalized
            .saturating_add(graph_degree.saturating_mul(5))
            .saturating_add(symbol_count.saturating_mul(7)),
    )
}

fn clamp_score(value: u32) -> u32 {
    value.min(1_000)
}

fn saturating_u32_from_usize(value: usize) -> u32 {
    u32::try_from(value).map_or(u32::MAX, std::convert::identity)
}

fn score_from_millis(final_score_millis: u32) -> f32 {
    let bounded_score =
        u16::try_from(clamp_score(final_score_millis)).map_or(u16::MAX, std::convert::identity);
    f32::from(bounded_score) / 1_000.0
}

fn authority_artifact_id(snapshot_id: &str, subject_identity: &str) -> String {
    format!("repo-map:{snapshot_id}:{subject_identity}")
}

fn build_search_text(parts: Vec<String>) -> String {
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
