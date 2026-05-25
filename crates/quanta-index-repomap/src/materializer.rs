use std::collections::BTreeMap;

use quanta_index_contract::{
    RepoMapChunkRecordDto, RepoMapFileIndexRecord, RepoMapGraphEdgeDto, RepoMapSnapshotMeta,
    RepoMapSourceBundle, RepoMapSymbolRecordDto,
};

use crate::model::{RepoMapEntryV1, RepoMapSnapshotV1};

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
    exactness_markers: Vec<String>,
}

impl RepoMapMaterializer {
    #[must_use]
    pub fn materialize(bundle: &RepoMapSourceBundle) -> RepoMapSnapshotV1 {
        let snapshot_meta = RepoMapSnapshotMeta {
            snapshot_id: bundle.snapshot_id.clone(),
            projection_version: bundle.projection_version,
            authority_digest: bundle.authority_digest.clone(),
            item_index_availability: bundle.item_index_availability.clone(),
            graph_coverage_class: bundle.graph_coverage_class.clone(),
            exactness_summary: bundle.exactness_summary.clone(),
        };
        let projection_status = projection_status(bundle);
        let call_stats = graph_stats(&bundle.call_edges);
        let import_stats = graph_stats(&bundle.import_edges);
        let (subject_chunks, owner_chunks) = chunk_stats(&bundle.chunk_records);
        let empty_graph_stats = GraphStatsV1::default();
        let empty_chunk_stats = ChunkStatsV1::default();

        let mut entries = Vec::new();
        for file in &bundle.file_indices {
            let file_call_stats = call_stats
                .get(&file.file_identity)
                .unwrap_or(&empty_graph_stats);
            let file_import_stats = import_stats
                .get(&file.file_identity)
                .unwrap_or(&empty_graph_stats);
            let file_chunk_stats = owner_chunks
                .get(&file.file_path)
                .unwrap_or(&empty_chunk_stats);
            entries.push(build_file_entry(
                bundle,
                file,
                &projection_status,
                file_call_stats,
                file_import_stats,
                file_chunk_stats,
            ));
            for symbol in &file.symbol_records {
                let symbol_call_stats = call_stats
                    .get(&symbol.subject_identity)
                    .unwrap_or(&empty_graph_stats);
                let symbol_import_stats = import_stats
                    .get(&symbol.subject_identity)
                    .unwrap_or(&empty_graph_stats);
                let subject_chunk = subject_chunks
                    .get(&symbol.subject_identity)
                    .or_else(|| owner_chunks.get(&symbol.owner_path))
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
        }

        entries.sort_by(|lhs, rhs| {
            rhs.final_score_millis
                .cmp(&lhs.final_score_millis)
                .then(lhs.subject_identity.cmp(&rhs.subject_identity))
        });
        for (index, entry) in entries.iter_mut().enumerate() {
            entry.rank = saturating_u32_from_usize(index.saturating_add(1));
            entry.included = true;
        }

        RepoMapSnapshotV1 {
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
            manifest_generation: bundle.manifest_generation,
            snapshot_meta,
            entries,
        }
    }
}

fn build_file_entry(
    bundle: &RepoMapSourceBundle,
    file: &RepoMapFileIndexRecord,
    projection_status: &str,
    call_stats: &GraphStatsV1,
    import_stats: &GraphStatsV1,
    chunk_stats: &ChunkStatsV1,
) -> RepoMapEntryV1 {
    let symbol_count = saturating_u32_from_usize(file.symbol_records.len());
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
    let owner_path = file.file_path.clone();
    let search_text = build_search_text(vec![
        file.file_identity.clone(),
        file.file_path.clone(),
        file.file_kind.clone(),
        file.symbol_records
            .iter()
            .map(|record| record.symbol_name.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        chunk_stats.preview_fragments.join(" "),
        chunk_stats.exactness_markers.join(" "),
        bundle.item_index_availability.clone(),
        bundle.graph_coverage_class.clone(),
    ]);
    RepoMapEntryV1 {
        subject_identity: file.file_identity.clone(),
        subject_doc_type: "File".to_string(),
        subject_kind: file.file_kind.clone(),
        owner_path,
        score: score_from_millis(final_score_millis),
        final_score_millis,
        included: true,
        rank: 0,
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
            &file.file_identity,
        ),
        projection_authority_digest: bundle.authority_digest.clone(),
        projection_status: projection_status.to_string(),
        redaction_state: bundle.redaction_state.clone(),
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
    symbol: &RepoMapSymbolRecordDto,
    projection_status: &str,
    call_stats: &GraphStatsV1,
    import_stats: &GraphStatsV1,
    chunk_stats: &ChunkStatsV1,
) -> RepoMapEntryV1 {
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
        symbol.symbol_name.clone(),
        symbol.subject_kind.clone(),
        symbol.owner_path.clone(),
        chunk_stats.preview_fragments.join(" "),
        chunk_stats.exactness_markers.join(" "),
        bundle.exactness_summary.clone(),
    ]);
    RepoMapEntryV1 {
        subject_identity: symbol.subject_identity.clone(),
        subject_doc_type: symbol.subject_doc_type.clone(),
        subject_kind: symbol.subject_kind.clone(),
        owner_path: symbol.owner_path.clone(),
        score: score_from_millis(final_score_millis),
        final_score_millis,
        included: true,
        rank: 0,
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
        redaction_state: bundle.redaction_state.clone(),
        search_text,
        source_symbol_count: 1,
        source_chunk_token_total: chunk_stats.token_total,
        source_call_incoming_edges: call_stats.incoming,
        source_call_outgoing_edges: call_stats.outgoing,
        source_import_incoming_edges: import_stats.incoming,
        source_import_outgoing_edges: import_stats.outgoing,
    }
}

fn graph_stats(edges: &[RepoMapGraphEdgeDto]) -> BTreeMap<String, GraphStatsV1> {
    let mut stats = BTreeMap::<String, GraphStatsV1>::new();
    for edge in edges {
        let from = stats.entry(edge.from_identity.clone()).or_default();
        from.outgoing = from.outgoing.saturating_add(1);
        let to = stats.entry(edge.to_identity.clone()).or_default();
        to.incoming = to.incoming.saturating_add(1);
    }
    stats
}

fn chunk_stats(
    chunks: &[RepoMapChunkRecordDto],
) -> (
    BTreeMap<String, ChunkStatsV1>,
    BTreeMap<String, ChunkStatsV1>,
) {
    let mut by_subject = BTreeMap::<String, ChunkStatsV1>::new();
    let mut by_owner = BTreeMap::<String, ChunkStatsV1>::new();
    for chunk in chunks {
        accumulate_chunk(
            by_subject
                .entry(chunk.subject_identity.clone())
                .or_default(),
            chunk,
        );
        accumulate_chunk(by_owner.entry(chunk.owner_path.clone()).or_default(), chunk);
    }
    (by_subject, by_owner)
}

fn accumulate_chunk(stats: &mut ChunkStatsV1, chunk: &RepoMapChunkRecordDto) {
    stats.token_total = stats.token_total.saturating_add(chunk.token_count);
    if !chunk.preview_text.trim().is_empty() {
        stats
            .preview_fragments
            .push(chunk.preview_text.trim().to_string());
    }
    if !chunk.exactness.trim().is_empty() {
        stats
            .exactness_markers
            .push(chunk.exactness.trim().to_string());
    }
}

fn total_degree(stats: &GraphStatsV1) -> u32 {
    stats.incoming.saturating_add(stats.outgoing)
}

fn freshness_score(bundle: &RepoMapSourceBundle) -> u32 {
    let item_index = bundle.item_index_availability.to_ascii_lowercase();
    let coverage = bundle.graph_coverage_class.to_ascii_lowercase();
    if (item_index.contains("available") || item_index.contains("full"))
        && (coverage.contains("complete") || coverage.contains("full"))
    {
        return 860;
    }
    if item_index.contains("partial") || coverage.contains("partial") {
        return 620;
    }
    500
}

fn projection_status(bundle: &RepoMapSourceBundle) -> String {
    let item_index = bundle.item_index_availability.to_ascii_lowercase();
    let coverage = bundle.graph_coverage_class.to_ascii_lowercase();
    if (item_index.contains("available") || item_index.contains("full"))
        && (coverage.contains("complete") || coverage.contains("full"))
    {
        return "Complete".to_string();
    }
    "Partial".to_string()
}

fn exactness_signal(bundle: &RepoMapSourceBundle, chunk_stats: &ChunkStatsV1) -> u32 {
    let mut score: u32 = if bundle
        .exactness_summary
        .to_ascii_lowercase()
        .contains("exact")
    {
        140
    } else {
        40
    };
    if chunk_stats
        .exactness_markers
        .iter()
        .any(|marker| marker.to_ascii_lowercase().contains("exact"))
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
