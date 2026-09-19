//! The ANN rail: recall@k, latency, build time and index bytes of a sealed
//! semantic generation, as a current-head artifact (QI-BB-027 완료 기준 #3,
//! QI-BB-031 완료 기준 #3).
//!
//! The rail seals one generation of `rows` pseudo-random unit directions in
//! `width` dimensions straight through the semantic adapter — the
//! production build, seal and open, with no daemon in between — and asks it
//! `queries` neighbour queries: half near a row (a paraphrase), half fresh.
//! Each answer is scored against an exhaustive cosine oracle computed here,
//! without the library, and timed. Random directions carry no cluster
//! structure for the graph to exploit, the hardest case for an approximate
//! index at this row count.
//!
//! The artifact is the one `BenchArtifactV1` envelope: the exact head, the
//! corpus and configuration digests, the host and the process's peak RSS,
//! the build phase, one row carrying the latency percentiles, and under
//! `detail` the recall, the dense lane the seal proved (index kind,
//! partitions, effort, lineage, segments), the normalization policy the rows
//! and the queries were held to, whether every returned score was the exact
//! cosine, and the index bytes on disk. Recall below the floor, a returned
//! score that is not the exact cosine, or a page shorter than `k` fails the
//! rail.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result as AnyResult, anyhow};
use quanta_index_contract::{
    EmbeddingNormalization, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
};
use quanta_index_core::{GenerationStorageKeyV1, RequestBudgetV1, SemanticIndexOpenPort};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, legacy_chunk_embedding_record_v1,
    sealed_replace_batch_v1,
};

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest,
    corpus_digest, directory_bytes, saturating_u64,
};

/// The artifact family this rail writes.
pub const DIMENSION: &str = "ann";

/// How far a returned score may be from the exact cosine: the sealed
/// effort refines the top candidates against the stored vectors, so a
/// returned score is the exact cosine up to `f32` rounding.
const EXACT_SCORE_TOLERANCE: f32 = 1e-4;

/// The one path every row is stored under.
const CORPUS_PATH: &str = "src/corpus.rs";

/// What the rail seals and asks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnnRailConfig {
    /// Rows sealed into the generation.
    pub rows: u64,
    /// Dimensions of every vector.
    pub width: usize,
    /// Neighbour queries asked.
    pub queries: u64,
    /// Neighbours asked per query, and the `k` of recall@k.
    pub k: usize,
    /// The recall@k the rail refuses to fall below.
    pub recall_floor: f64,
}

impl AnnRailConfig {
    /// The tier the seal-time recall gate pins (§3.23): 4,096 rows of 64
    /// dimensions, 64 queries, recall@10 of at least 0.95.
    pub const DEFAULT: Self = Self {
        rows: 4_096,
        width: 64,
        queries: 64,
        k: 10,
        recall_floor: 0.95,
    };
}

/// What one run measured.
#[derive(Clone, Debug)]
pub struct AnnReport {
    pub config: AnnRailConfig,
    /// Mean over the queries of the share of the exact top-k the index
    /// returned.
    pub recall_at_k: f64,
    /// Wall time of each query, in milliseconds, in query order.
    pub latencies_ms: Vec<f64>,
    /// Build through seal, in milliseconds.
    pub build_ms: f64,
    /// Bytes of the index files the seal left under the dataset.
    pub index_bytes: u64,
    /// Bytes of the whole sealed generation directory.
    pub generation_bytes: u64,
    /// The dense lane the open proved, as the planner traces it.
    pub dense_lane: String,
    /// The normalization the rows were sealed under and every query was
    /// held to.
    pub normalization: EmbeddingNormalization,
    /// Every returned score was the exact cosine of its row.
    pub exact_scores: bool,
    /// Queries whose page was shorter than `k`.
    pub short_pages: u64,
    pub passed: bool,
}

/// One deterministic unit direction per seed.
///
/// A xorshift walk mapped to `[-1, 1)` per lane, then normalized. The
/// seal-time recall gate draws its corpus from the same walk, so the two
/// measure the same space.
#[must_use]
pub fn unit_direction(seed: u64, width: usize) -> Vec<f32> {
    let mut state = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xD1B5_4A32_D192_ED03);
    let mut vector = Vec::with_capacity(width);
    for _ in 0..width {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let [low, high, ..] = state.to_le_bytes();
        let lane = f32::from(u16::from_le_bytes([low, high]));
        vector.push(lane / 32_768.0_f32 - 1.0);
    }
    unit(&vector)
}

fn unit(vector: &[f32]) -> Vec<f32> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    vector.iter().map(|value| value / norm).collect()
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

/// Query `seed`: an even seed sits near a row (the row plus 0.35 of a
/// fresh direction, renormalized), an odd one is a fresh direction.
fn query_direction(seed: u64, config: &AnnRailConfig) -> AnyResult<Vec<f32>> {
    if seed.is_multiple_of(2) {
        let row = seed
            .wrapping_mul(61)
            .checked_rem(config.rows)
            .context("the rail seals at least one row")?;
        let near = unit_direction(row, config.width);
        let noise = unit_direction(1_000_000_u64.wrapping_add(seed), config.width);
        let mixed: Vec<f32> = near
            .iter()
            .zip(&noise)
            .map(|(row, drift)| row + 0.35 * drift)
            .collect();
        Ok(unit(&mixed))
    } else {
        Ok(unit_direction(
            2_000_000_u64.wrapping_add(seed),
            config.width,
        ))
    }
}

/// Exact top-k by cosine over `rows`, ties broken by id: the oracle.
fn exact_top_k(rows: &[(String, Vec<f32>)], query: &[f32], k: usize) -> Vec<String> {
    let mut scored: Vec<(&str, f32)> = rows
        .iter()
        .map(|(id, vector)| (id.as_str(), cosine(vector, query)))
        .collect();
    scored.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    scored
        .into_iter()
        .take(k)
        .map(|(id, _)| id.to_string())
        .collect()
}

fn repo() -> RepoId {
    RepoId::new("ann-rail")
}

fn revision() -> RevisionId {
    RevisionId::new("ann-rail-rev")
}

/// The id of row `seed`.
fn row_id(seed: u64) -> String {
    format!("row-{seed}")
}

/// Bytes of every file under the dataset's index directory.
fn index_bytes(generation_dir: &Path) -> AnyResult<u64> {
    let mut total = 0_u64;
    let mut pending = vec![generation_dir.join("dataset")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .with_context(|| format!("list {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else if path
                .components()
                .any(|component| component.as_os_str() == "_indices")
            {
                total = total.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(total)
}

/// Seal the corpus under `state_root`, ask every query, and score it.
pub fn run_ann_report(config: AnnRailConfig, state_root: &Path) -> AnyResult<AnnReport> {
    let width = u32::try_from(config.width).context("the width fits a contract dimension")?;
    let adapter = SemanticAdapter::with_state_root(state_root.to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    let rows: Vec<(String, Vec<f32>)> = (0..config.rows)
        .map(|seed| (row_id(seed), unit_direction(seed, config.width)))
        .collect();
    let embeddings = rows
        .iter()
        .map(|(id, vector)| legacy_chunk_embedding_record_v1(id, CORPUS_PATH, vector.clone()))
        .collect::<Result<Vec<_>, String>>()
        .map_err(|error| anyhow!("ann rail row: {error}"))?;
    let batch = sealed_replace_batch_v1(
        repo(),
        revision(),
        generation,
        CORPUS_PATH,
        embeddings,
        width,
    );
    let normalization = batch.model_contract.normalization;
    let build_started = Instant::now();
    build_resident_batch_v1(&adapter, &batch)?;
    let build_ms = build_started.elapsed().as_secs_f64() * 1_000.0;
    let generation_dir = GenerationStorageKeyV1::for_repo_revision(&repo(), &revision())
        .generation_dir(state_root, generation);

    let searcher = adapter.open_proven(&GenerationSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        track: quanta_index_contract::SearchPlaneTrackKind::Semantic,
        manifest_generation: generation,
        manifest_digest: batch.manifest_digest,
    })?;
    let k = u32::try_from(config.k).context("k fits a top_k")?;
    let budget = RequestBudgetV1::unbounded();
    let mut recall_sum = 0.0_f64;
    let mut latencies_ms = Vec::new();
    let mut exact_scores = true;
    let mut short_pages = 0_u64;
    for seed in 0..config.queries {
        let query = query_direction(seed, &config)?;
        let expected = exact_top_k(&rows, &query, config.k);
        let started = Instant::now();
        let hits = searcher.search(&query, k, &budget)?;
        latencies_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        if hits.len() < config.k {
            short_pages = short_pages.saturating_add(1);
        }
        let mut found = 0_u32;
        for hit in &hits {
            let row = rows
                .iter()
                .find(|(id, _)| *id == hit.candidate_id)
                .ok_or_else(|| {
                    anyhow!(
                        "the index returned {}, which is not a row",
                        hit.candidate_id
                    )
                })?;
            if (hit.score - cosine(&row.1, &query)).abs() > EXACT_SCORE_TOLERANCE {
                exact_scores = false;
            }
            if expected.contains(&hit.candidate_id) {
                found = found.saturating_add(1);
            }
        }
        recall_sum += f64::from(found) / f64::from(k);
    }
    let queries = u32::try_from(config.queries).context("the query count fits u32")?;
    let recall_at_k = recall_sum / f64::from(queries);
    let passed = recall_at_k >= config.recall_floor && exact_scores && short_pages == 0;
    Ok(AnnReport {
        config,
        recall_at_k,
        latencies_ms,
        build_ms,
        index_bytes: index_bytes(&generation_dir)?,
        generation_bytes: directory_bytes(&generation_dir)?,
        dense_lane: searcher.dense_lane().trace_detail(),
        normalization,
        exact_scores,
        short_pages,
        passed,
    })
}

/// The normalization policy as the manifest spells it.
const fn normalization_name(normalization: EmbeddingNormalization) -> &'static str {
    match normalization {
        EmbeddingNormalization::None => "none",
        EmbeddingNormalization::L2Unit => "l2_unit",
    }
}

fn config_entries(config: &AnnRailConfig) -> [(&'static str, String); 5] {
    [
        ("rows", config.rows.to_string()),
        ("width", config.width.to_string()),
        ("queries", config.queries.to_string()),
        ("k", config.k.to_string()),
        ("recall_floor", config.recall_floor.to_string()),
    ]
}

/// The corpus is its generator and size: the digest names the walk, the
/// row count and the width, which fix every byte the rail sealed.
fn corpus_files(config: &AnnRailConfig) -> Vec<(String, String)> {
    vec![(
        CORPUS_PATH.to_string(),
        format!(
            "unit_direction:xorshift-9e3779b97f4a7c15-d1b54a32d192ed03:rows={}:width={}",
            config.rows, config.width
        ),
    )]
}

/// The one `BenchArtifactV1` for `report`.
pub fn artifact(
    report: &AnnReport,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    let config = &report.config;
    let latency = LatencySummary::from_samples_ms(&report.latencies_ms);
    let row = BenchRowV1 {
        scenario_id: format!("ann.recall_at_{}", config.k),
        // The family of the result shape (chunk candidates), as the judged
        // relevance routes record it; the dense lane is named in
        // `engine_touched`.
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        result_shape: ResultShape::Candidates,
        latency,
        qps: None,
        error_count: report.short_pages,
        timeout_count: 0,
        result_count: Some(saturating_u64(config.k)),
        typed_error_code: None,
        engine_touched: vec!["semantic".to_string(), "dense".to_string()],
        early_stop_reason: None,
    };
    Ok(BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest(DIMENSION, &corpus_files(config)),
            config_digest: config_digest(DIMENSION, &config_entries(config)),
            model_revision: None,
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1 {
            build_ms: Some(report.build_ms),
            update_ms: None,
            gc_ms: None,
        },
        disk_amplification: None,
        rows: vec![row],
        detail: serde_json::json!({
            "rows": config.rows,
            "width": config.width,
            "queries": config.queries,
            "k": config.k,
            "recall_at_k": report.recall_at_k,
            "recall_floor": config.recall_floor,
            "exact_scores": report.exact_scores,
            "short_pages": report.short_pages,
            "normalization": normalization_name(report.normalization),
            "dense_lane": report.dense_lane,
            "index_bytes": report.index_bytes,
            "generation_bytes": report.generation_bytes,
            "provider": "synthetic unit directions (no embedding model)",
            "passed": report.passed,
        }),
    })
}

/// Write `summary.json` (the `BenchArtifactV1`) under `dir`.
pub fn write_artifacts(
    report: &AnnReport,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<()> {
    artifact(report, git_head, host)?.write_to(&dir.join("summary.json"))?;
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index the JSON this module constructs; a missing key is a test failure"
)]
mod tests {
    use super::*;

    /// A tier at the index floor: 300 rows is above the 256-row threshold,
    /// so the seal builds the ANN index the full tier measures.
    const SMALL: AnnRailConfig = AnnRailConfig {
        rows: 300,
        width: 16,
        queries: 8,
        k: 5,
        recall_floor: 0.0,
    };

    /// The walk is deterministic and lands on the unit sphere.
    #[test]
    fn a_direction_is_deterministic_and_unit() {
        let first = unit_direction(7, 64);
        assert_eq!(first, unit_direction(7, 64));
        assert_ne!(first, unit_direction(8, 64));
        let norm = first
            .iter()
            .map(|value| f64::from(*value) * f64::from(*value))
            .sum::<f64>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm {norm}");
    }

    /// The oracle ranks by cosine and breaks ties by id.
    #[test]
    fn the_oracle_ranks_by_cosine_then_id() {
        let rows = vec![
            ("b".to_string(), vec![1.0, 0.0]),
            ("a".to_string(), vec![1.0, 0.0]),
            ("c".to_string(), vec![0.0, 1.0]),
        ];
        assert_eq!(exact_top_k(&rows, &[1.0, 0.0], 2), vec!["a", "b"]);
        assert_eq!(exact_top_k(&rows, &[0.0, 1.0], 1), vec!["c"]);
    }

    /// A small tier's run is exact, full and complete.
    ///
    /// It seals through the index, returns exact scores and full pages,
    /// records every query's latency and the `l2_unit` policy, and its
    /// artifact carries every field the artifact gate requires.
    #[test]
    fn a_small_tier_measures_and_writes_a_complete_artifact() {
        let temp = tempfile::tempdir().expect("tempdir");
        let report = run_ann_report(SMALL, temp.path()).expect("the rail runs");
        assert_eq!(report.latencies_ms.len(), 8);
        assert!(
            report.exact_scores,
            "the sealed effort refines to exact cosine"
        );
        assert_eq!(report.short_pages, 0);
        assert!((0.0..=1.0).contains(&report.recall_at_k));
        assert!(report.index_bytes > 0, "300 rows seal an ANN index");
        assert!(report.generation_bytes > report.index_bytes);
        assert!(
            report.dense_lane.contains("dense.index=ivf_hnsw_sq"),
            "{}",
            report.dense_lane
        );
        assert_eq!(report.normalization, EmbeddingNormalization::L2Unit);
        assert!(report.passed);

        let head = GitHeadV1::parse(&"a".repeat(40)).expect("a full head");
        let host = HostV1::observe().expect("the host");
        let json = artifact(&report, head, host)
            .expect("artifact")
            .to_json()
            .expect("json");
        assert_eq!(json["schema_version"], 2);
        assert_eq!(json["dimension"], DIMENSION);
        assert_eq!(json["detail"]["normalization"], "l2_unit");
        assert_eq!(json["detail"]["k"], 5);
        assert!(
            json["phases"]["build_ms"]
                .as_f64()
                .is_some_and(|ms| ms > 0.0)
        );
        let latency = &json["rows"][0]["latency"];
        for key in ["p50_ms", "p95_ms", "p99_ms", "samples"] {
            assert!(!latency[key].is_null(), "latency.{key}");
        }
        assert_eq!(latency["samples"], 8);
        for key in ["corpus_digest", "config_digest"] {
            assert!(
                json["provenance"][key]
                    .as_str()
                    .is_some_and(|digest| digest.starts_with("sha256:")),
                "{key}"
            );
        }
    }
}
