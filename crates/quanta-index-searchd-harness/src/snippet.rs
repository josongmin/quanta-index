//! Snippet-quality rail (J7Q-02).
//!
//! Measures the lexical snippet beyond mere substring-presence. A snippet that
//! merely *contains* the matched needle is not a quality snippet: a useful
//! snippet is a **hit-centered window** — the needle sits inside a bounded slice
//! of surrounding context, and an over-long source line is truncated
//! deterministically so the head of the result list never streams an unbounded
//! blob. This rail seeds a corpus designed to exercise four snippet intents
//! (phrase hit, regex hit, multi-hit, long-line), runs each query against the
//! live ranker, and grades the produced excerpt against
//! three blocking properties:
//!
//! 1. **needle present** — the matched token/phrase actually appears in the
//!    emitted snippet (substring presence is necessary but not sufficient);
//! 2. **hit centered** — for a long source line, the hit must not be pinned to
//!    an extreme edge of the window (a window that starts exactly at the hit and
//!    runs off the end gives no leading context);
//! 3. **bounded window** — the emitted snippet length is within a declared
//!    ceiling, i.e. a long line is truncated deterministically.
//!
//! The rail is split into two layers that are tested independently:
//!
//! - a **pure window oracle** ([`compute_snippet_window`]) that derives the
//!   expected hit-centered window from the fixed ingested source, independent
//!   of the daemon's emitted snippet;
//! - a **seeded end-to-end measurement** that records the engine's ACTUAL
//!   snippet against that contract.
//!
//! Fail-closed posture: the rail does NOT rewrite or post-process the engine
//! snippet to make it pass. The live lexical path truncates an over-long source
//! line to a hit-centered, bounded window (the J7Q-02 engine fix), and this rail
//! compares that emitted window with the fixed source oracle. A wrong-source
//! snippet containing the needle or an unwindowed long line trips the gate.

use std::path::Path;

use anyhow::Result as AnyResult;
use quanta_index_contract::HighlightSpan;
use quanta_index_contract::{
    index::{SourceCut, ViewRef},
    native_protocol::NativeTextPreviewEvidence,
};
use serde_json::{Value, json};

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest, corpus_digest,
    saturating_u64,
};

pub use crate::snippet_fixture::{
    MAX_SNIPPET_LEN, MIN_LEADING_CONTEXT, SNIPPET_CORPUS, SNIPPET_QUERIES, SNIPPET_REPO,
    SnippetFixture, SnippetIntent, SnippetQuery, TOP_K,
};

/// Lossless-in-practice `usize -> f64` for snippet offsets and lengths.
///
/// Snippet byte offsets are far below `2^53`, so the `cast_precision_loss` the
/// workspace denies cannot occur on these values; the localized expect
/// documents that invariant rather than hiding it.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "snippet byte offsets and lengths are far below 2^53, so usize->f64 is exact on these values"
)]
fn usize_to_f64(n: usize) -> f64 {
    n as f64
}

// ---------------------------------------------------------------------------
// Pure window oracle (the target snippet contract).
// ---------------------------------------------------------------------------

/// A hit-centered, bounded, deterministically-truncated snippet window.
///
/// Returned by [`compute_snippet_window`]. `text` is the windowed slice;
/// `hit_offset` is the byte offset of the needle *within* `text` (not within the
/// original source).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnippetWindow {
    pub text: String,
    pub hit_offset: usize,
}

/// Compute the ideal hit-centered window for `needle` inside `source`.
///
/// Contract (deterministic, no I/O, no randomness):
/// - if the source already fits within `max_len`, the whole source is the
///   window (no truncation, hit offset = first match offset);
/// - otherwise a window of at most `max_len` bytes is centered on the first
///   match so the needle has leading *and* trailing context where the source
///   permits, clamped to a UTF-8 char boundary so the result is always valid
///   UTF-8;
/// - returns `None` when the needle is absent or too wide for the window.
///
/// Truncation is byte-window-based and fully determined by
/// `(source, needle, max_len)`, so two runs over identical inputs always emit
/// byte-identical windows — the property the golden-window artifact freezes.
#[must_use]
pub fn compute_snippet_window(source: &str, needle: &str, max_len: usize) -> Option<SnippetWindow> {
    if needle.is_empty() || needle.len() > max_len {
        return None;
    }
    let hit = source.find(needle)?;
    if source.len() <= max_len {
        return Some(SnippetWindow {
            text: source.to_string(),
            hit_offset: hit,
        });
    }
    // Leave room for the entire hit before assigning leading context. The
    // first boundary at or after the ideal start preserves that space.
    let lead = half(max_len).min(max_len.checked_sub(needle.len())?);
    let ideal_start = hit.saturating_sub(lead);
    let start = (ideal_start..=hit).find(|offset| source.is_char_boundary(*offset))?;
    let end = floor_char_boundary(source, start.saturating_add(max_len));
    let text = source.get(start..end)?.to_string();
    let hit_offset = hit.checked_sub(start)?;
    Some(SnippetWindow { text, hit_offset })
}

/// Integer halving of a byte budget into a leading-context allowance.
///
/// The remainder (when `n` is odd) deliberately accrues to the trailing side, so
/// the split is total and deterministic; precision is irrelevant for a byte
/// budget that is always a small whole number.
#[expect(
    clippy::integer_division,
    reason = "byte budgets are whole numbers; the odd remainder is intentionally given to the trailing side"
)]
fn half(n: usize) -> usize {
    n / 2
}

/// Step `index` down to the nearest UTF-8 char boundary at or below it.
///
/// `str::floor_char_boundary` is unstable, so this is a stable hand-rolled
/// equivalent. `index` is always clamped into `0..=len` by callers.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    let mut i = index.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i = i.saturating_sub(1);
    }
    i
}

// ---------------------------------------------------------------------------
// Observed snippet metrics + gate.
// ---------------------------------------------------------------------------

/// Per-snippet observed metrics, computed from the engine's emitted snippet.
#[derive(Clone, Debug, PartialEq)]
pub struct SnippetMetrics {
    /// The matched needle is a substring of the emitted snippet.
    pub needle_present: bool,
    /// Byte length of the emitted snippet.
    pub window_len: usize,
    /// Byte offset of the FIRST needle hit within the emitted snippet, or
    /// `None` when the needle is absent.
    pub hit_offset_within_window: Option<usize>,
    /// Number of (non-overlapping) needle occurrences in the emitted snippet.
    pub multi_hit_count: usize,
    /// Centering ratio in `[0.0, 1.0]`: `hit_offset / window_len`. `0.0` means
    /// the hit sits at the leading edge; `~0.5` is well centered. `None` when
    /// the needle is absent or the window is empty.
    pub center_ratio: Option<f64>,
}

/// Measure the observed snippet against the needle. Pure: no I/O, no ranker.
#[must_use]
pub fn measure_snippet(snippet: &str, needle: &str) -> SnippetMetrics {
    let window_len = snippet.len();
    let multi_hit_count = if needle.is_empty() {
        0
    } else {
        snippet.matches(needle).count()
    };
    let hit_offset_within_window = if needle.is_empty() {
        None
    } else {
        snippet.find(needle)
    };
    let needle_present = hit_offset_within_window.is_some();
    let center_ratio = match hit_offset_within_window {
        Some(offset) if window_len > 0 => Some(usize_to_f64(offset) / usize_to_f64(window_len)),
        _ => None,
    };
    SnippetMetrics {
        needle_present,
        window_len,
        hit_offset_within_window,
        multi_hit_count,
        center_ratio,
    }
}

/// Enforce the blocking snippet-quality gate for one observed snippet, given the
/// declared intent. Collects every violation (never short-circuits) so the
/// artifact records the full failure set.
#[must_use]
pub fn gate_snippet(
    intent: SnippetIntent,
    metrics: &SnippetMetrics,
    max_len: usize,
    min_leading_context: usize,
) -> Vec<String> {
    let mut failures = Vec::new();

    // 1. Needle present — the universal floor.
    if !metrics.needle_present {
        failures.push(format!(
            "{}: needle absent from emitted snippet (substring presence floor failed)",
            intent.as_str()
        ));
        // Without a hit, centering/bound checks are vacuous; return early so the
        // failure set is the single load-bearing one.
        return failures;
    }

    // 2. Multi-hit intents must observe more than one occurrence.
    if matches!(intent, SnippetIntent::MultiHit) && metrics.multi_hit_count < 2 {
        failures.push(format!(
            "{}: expected multiple needle occurrences, observed {}",
            intent.as_str(),
            metrics.multi_hit_count
        ));
    }

    // 3. Bounded window — long-line intents MUST be truncated below the ceiling.
    if intent.requires_truncation() && metrics.window_len > max_len {
        failures.push(format!(
            "{}: snippet length {} exceeds bound {max_len} (long line not deterministically truncated)",
            intent.as_str(),
            metrics.window_len
        ));
    }

    // 4. Hit centered — for a truncation-requiring intent, the hit must carry
    //    leading context; a hit pinned to the leading edge means the window was
    //    not centered.
    if intent.requires_truncation()
        && let Some(offset) = metrics.hit_offset_within_window
        && offset < min_leading_context
    {
        failures.push(format!(
            "{}: hit at offset {offset} is within {min_leading_context}B of the leading edge (window not hit-centered)",
            intent.as_str()
        ));
    }

    failures
}

// ---------------------------------------------------------------------------
// Seeded corpus + judged snippet queries.
// ---------------------------------------------------------------------------

/// Scored outcome for one snippet query.
#[derive(Clone, Debug)]
pub struct SnippetScore {
    pub id: &'static str,
    pub intent: SnippetIntent,
    pub query: &'static str,
    pub needle: &'static str,
    pub expect_path: &'static str,
    /// The path of the candidate whose snippet was graded, if a candidate for
    /// the expected path was returned.
    pub matched_path: Option<String>,
    /// The exact snippet bytes the engine emitted (the golden window record).
    pub snippet: Option<String>,
    pub metrics: Option<SnippetMetrics>,
    pub observed_hit_offset: Option<u32>,
    pub observed_highlights: Vec<HighlightSpan>,
    /// Actual ranked rows returned by the engine, including decoys.
    pub ranked_hits: Option<u64>,
    /// Full verified Native match count, or the Legacy candidate count.
    pub matched_total: Option<u64>,
    pub failures: Vec<String>,
}

impl SnippetScore {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// The full snippet report.
#[derive(Clone, Debug)]
pub struct SnippetReport {
    pub scores: Vec<SnippetScore>,
    pub passed: bool,
    pub native_provenance: Option<NativeSnippetProvenance>,
}

/// Source and executable identity for an actual Native preview observation.
#[derive(Clone, Debug)]
pub struct NativeSnippetProvenance {
    pub published_view: ViewRef,
    pub source_cut: SourceCut,
    pub config_sha256: String,
    pub config_template_sha256: String,
    pub daemon_sha256: String,
}

/// Resolve the immutable source bytes by the fixture's expected path, never
/// by a returned snippet or by the candidate's claimed source metadata.
fn fixture_source(expect_path: &str) -> Option<&'static str> {
    SNIPPET_CORPUS
        .iter()
        .find(|fixture| fixture.path == expect_path)
        .map(|fixture| fixture.content)
}

fn fixture_window(expect_path: &str, needle: &str) -> Option<SnippetWindow> {
    compute_snippet_window(fixture_source(expect_path)?, needle, MAX_SNIPPET_LEN)
}

/// The emitted bytes must come from the fixed source at the declared focus.
/// Presence, length and centering alone permit unrelated text around a needle.
fn fixture_window_failures(
    query: &SnippetQuery,
    observed: &str,
    hit_offset: Option<u32>,
    highlights: &[HighlightSpan],
) -> Vec<String> {
    let expect_path = query.expect_path;
    let Some(expected) = fixture_window(expect_path, query.needle) else {
        return vec![format!(
            "fixed source or needle missing for `{expect_path}`"
        )];
    };
    if observed != expected.text {
        return vec![format!(
            "fixed source window differs from emitted snippet for `{expect_path}`"
        )];
    }

    // These fixtures use literal ASCII needles with deliberately chosen token
    // boundaries. The regex additionally covers the full positive signature.
    let mut ranges: Vec<_> = expected
        .text
        .match_indices(query.needle)
        .map(|(start, text)| (start, text.len()))
        .collect();
    if query.intent == SnippetIntent::Regex {
        let signature = "regex_target_symbol(input: &str)";
        ranges.extend(
            expected
                .text
                .match_indices(signature)
                .map(|(start, text)| (start, text.len())),
        );
        ranges.sort_unstable();
    }
    let mut failures = Vec::new();
    if hit_offset.and_then(|offset| usize::try_from(offset).ok()) != Some(expected.hit_offset) {
        failures.push(format!(
            "fixed source hit offset differs for `{expect_path}`"
        ));
    }
    let observed_ranges = highlights.iter().map(|span| {
        usize::try_from(span.start)
            .ok()
            .zip(usize::try_from(span.len).ok())
    });
    if !observed_ranges.eq(ranges.into_iter().map(Some)) {
        failures.push(format!(
            "fixed source highlights differ for `{expect_path}`"
        ));
    }
    failures
}

/// The negative controls share the applicable posting anchors. Losing phrase
/// adjacency or regex verification must change the judged result population.
fn predicate_population_failures(intent: SnippetIntent, candidate_count: usize) -> Vec<String> {
    if matches!(intent, SnippetIntent::Phrase | SnippetIntent::Regex) && candidate_count != 1 {
        vec![format!(
            "{} fixture expected one selected source, observed {candidate_count}",
            intent.as_str()
        )]
    } else {
        Vec::new()
    }
}

/// Grade the exact emitted bytes against the fixed source.
#[expect(
    clippy::too_many_arguments,
    reason = "the scorer uses explicit query, emitted excerpt, rank count, and source evidence"
)]
fn score_observed(
    query: &SnippetQuery,
    matched_path: &str,
    snippet: &str,
    hit_offset: Option<u32>,
    highlights: &[HighlightSpan],
    candidate_count: usize,
    ranked_hits: usize,
    matched_total: u64,
) -> SnippetScore {
    let metrics = measure_snippet(snippet, query.needle);
    let mut failures = gate_snippet(query.intent, &metrics, MAX_SNIPPET_LEN, MIN_LEADING_CONTEXT);
    failures.extend(fixture_window_failures(
        query, snippet, hit_offset, highlights,
    ));
    failures.extend(predicate_population_failures(query.intent, candidate_count));
    SnippetScore {
        id: query.id,
        intent: query.intent,
        query: query.query,
        needle: query.needle,
        expect_path: query.expect_path,
        matched_path: Some(matched_path.to_string()),
        snippet: Some(snippet.to_string()),
        metrics: Some(metrics),
        observed_hit_offset: hit_offset,
        observed_highlights: highlights.to_vec(),
        ranked_hits: Some(saturating_u64(ranked_hits)),
        matched_total: Some(matched_total),
        failures,
    }
}

/// Grade the product Native SDK preview without rewriting its excerpt evidence.
pub fn run_native_snippet_report(daemon_bin: &Path) -> AnyResult<SnippetReport> {
    let observed = crate::native_index::run_native_snippet_observations(daemon_bin)?;
    anyhow::ensure!(
        observed.rows.len() == SNIPPET_QUERIES.len(),
        "Native snippet observation omitted judged queries"
    );
    let mut scores = Vec::with_capacity(SNIPPET_QUERIES.len());
    for (query, row) in SNIPPET_QUERIES.iter().zip(&observed.rows) {
        let candidate_count = usize::try_from(row.ranking.matched)?;
        let selected = row
            .ranking
            .hits
            .iter()
            .zip(&row.previews)
            .find(|(hit, _)| hit.path == query.expect_path);
        let score = match selected {
            Some((hit, preview)) => match &preview.evidence {
                NativeTextPreviewEvidence::Excerpt(excerpt) => {
                    let offset = u32::try_from(
                        excerpt.original_focus.start - excerpt.original_context.start,
                    )?;
                    score_observed(
                        query,
                        &hit.path,
                        &excerpt.text,
                        Some(offset),
                        &excerpt.highlights,
                        candidate_count,
                        row.ranking.hits.len(),
                        row.ranking.matched,
                    )
                }
                NativeTextPreviewEvidence::Path => {
                    anyhow::bail!(
                        "Native snippet {} returned path evidence for a body query",
                        query.id
                    )
                }
                NativeTextPreviewEvidence::Unavailable(reason) => SnippetScore {
                    id: query.id,
                    intent: query.intent,
                    query: query.query,
                    needle: query.needle,
                    expect_path: query.expect_path,
                    matched_path: Some(hit.path.clone()),
                    snippet: None,
                    metrics: None,
                    observed_hit_offset: None,
                    observed_highlights: Vec::new(),
                    ranked_hits: Some(u64::try_from(row.ranking.hits.len())?),
                    matched_total: Some(row.ranking.matched),
                    failures: {
                        let mut failures = vec![format!(
                            "{}: Native preview unavailable for `{}`: {reason:?}",
                            query.intent.as_str(),
                            query.expect_path
                        )];
                        failures
                            .extend(predicate_population_failures(query.intent, candidate_count));
                        failures
                    },
                },
            },
            None => SnippetScore {
                id: query.id,
                intent: query.intent,
                query: query.query,
                needle: query.needle,
                expect_path: query.expect_path,
                matched_path: None,
                snippet: None,
                metrics: None,
                observed_hit_offset: None,
                observed_highlights: Vec::new(),
                ranked_hits: Some(u64::try_from(row.ranking.hits.len())?),
                matched_total: Some(row.ranking.matched),
                failures: {
                    let mut failures = vec![format!(
                        "{}: expected candidate for `{}` not retrieved (snippet ungradable)",
                        query.intent.as_str(),
                        query.expect_path
                    )];
                    failures.extend(predicate_population_failures(query.intent, candidate_count));
                    failures
                },
            },
        };
        scores.push(score);
    }
    let passed = scores.iter().all(SnippetScore::passed);
    Ok(SnippetReport {
        scores,
        passed,
        native_provenance: Some(NativeSnippetProvenance {
            published_view: observed.published_view,
            source_cut: observed.source_cut,
            config_sha256: observed.config_sha256,
            config_template_sha256: observed.config_template_sha256,
            daemon_sha256: observed.daemon_sha256,
        }),
    })
}

// ---------------------------------------------------------------------------
// Artifact emission.
// ---------------------------------------------------------------------------

fn metrics_json(metrics: &SnippetMetrics) -> Value {
    json!({
        "needle_present": metrics.needle_present,
        "window_len": metrics.window_len,
        "hit_offset_within_window": metrics.hit_offset_within_window,
        "multi_hit_count": metrics.multi_hit_count,
        "center_ratio": metrics.center_ratio,
    })
}

fn score_json(score: &SnippetScore) -> Value {
    json!({
        "id": score.id,
        "intent": score.intent.as_str(),
        "query": score.query,
        "needle": score.needle,
        "expect_path": score.expect_path,
        "matched_path": score.matched_path,
        "metrics": score.metrics.as_ref().map(metrics_json),
        "observed_hit_offset": score.observed_hit_offset,
        "observed_highlights": score.observed_highlights,
        "ranked_hits": score.ranked_hits,
        "matched_total": score.matched_total,
        "bounds": {
            "max_snippet_len": MAX_SNIPPET_LEN,
            "min_leading_context": MIN_LEADING_CONTEXT,
            "requires_truncation": score.intent.requires_truncation(),
        },
        "failures": score.failures,
        "passed": score.passed(),
    })
}

/// The golden-window record for one query.
///
/// Pair the emitted bytes with the window computed from the fixed ingested
/// source. The expected window exists even when retrieval emits no snippet.
fn golden_window_json(score: &SnippetScore) -> Value {
    let observed = score.snippet.clone();
    let oracle = fixture_window(score.expect_path, score.needle);
    json!({
        "id": score.id,
        "intent": score.intent.as_str(),
        "needle": score.needle,
        "observed_snippet": observed,
        "observed_len": score.snippet.as_ref().map(String::len),
        "oracle_window": oracle.as_ref().map(|w| w.text.clone()),
        "oracle_hit_offset": oracle.as_ref().map(|w| w.hit_offset),
        "oracle_len": oracle.as_ref().map(|w| w.text.len()),
    })
}

fn artifact_detail(report: &SnippetReport) -> Value {
    let scores: Vec<Value> = report.scores.iter().map(score_json).collect();
    json!({
        "passed": report.passed,
        "backend": if report.native_provenance.is_some() { "index-native" } else { "legacy-reference" },
        "native_provenance": report.native_provenance.as_ref().map(|binding| json!({
            "published_view": binding.published_view,
            "source_cut": binding.source_cut,
            "config_sha256": binding.config_sha256,
            "config_template_sha256": binding.config_template_sha256,
            "daemon_sha256": binding.daemon_sha256,
            "resource_scope": "snippet_matrix process; daemon resources not sampled",
        })),
        "max_snippet_len": MAX_SNIPPET_LEN,
        "min_leading_context": MIN_LEADING_CONTEXT,
        "scores": scores,
        "gate_note": "snippet quality requires the fixed-source window, hit-centered bounded length, and deterministic truncation; the engine snippet is graded as-emitted, never post-processed to pass",
    })
}

/// The current-source snippet verdict envelope. The supplemental golden record
/// remains separate because it is a content oracle, not a second authority.
pub fn artifact(
    report: &SnippetReport,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    let corpus = SNIPPET_CORPUS
        .iter()
        .map(|fixture| (fixture.path.to_string(), fixture.content.to_string()))
        .collect::<Vec<_>>();
    let mut config_entries = vec![
        ("max_snippet_len", MAX_SNIPPET_LEN.to_string()),
        ("min_leading_context", MIN_LEADING_CONTEXT.to_string()),
        ("queries", SNIPPET_QUERIES.len().to_string()),
    ];
    if let Some(binding) = &report.native_provenance {
        config_entries.push(("backend", "index-native".to_string()));
        config_entries.push((
            "native_config_template_sha256",
            binding.config_template_sha256.clone(),
        ));
        config_entries.push(("native_daemon_sha256", binding.daemon_sha256.clone()));
    }
    Ok(BenchArtifactV1 {
        dimension: "snippet".to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest("snippet", &corpus),
            config_digest: config_digest("snippet", &config_entries),
            model_revision: None,
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows: report
            .scores
            .iter()
            .map(|score| BenchRowV1 {
                scenario_id: score.id.to_string(),
                route_family: RouteFamily::Lexical,
                syntax: BenchSyntax::Native,
                result_shape: if score.ranked_hits.is_some_and(|count| count > 0) {
                    ResultShape::Candidates
                } else {
                    ResultShape::Empty
                },
                latency: None,
                qps: None,
                error_count: saturating_u64(score.failures.len()),
                timeout_count: 0,
                result_count: score.ranked_hits,
                typed_error_code: None,
                engine_touched: vec!["lexical".to_string()],
                early_stop_reason: None,
            })
            .collect(),
        detail: artifact_detail(report),
    })
}

/// Write the authority envelope and the supplemental golden-window record.
pub fn write_artifacts(
    report: &SnippetReport,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<()> {
    std::fs::create_dir_all(dir)?;
    invalidate_artifacts(dir)?;
    let golden: Vec<Value> = report.scores.iter().map(golden_window_json).collect();
    let git_head_text = git_head.as_str().to_string();
    let summary = artifact(report, git_head, host)?;
    let golden_file = tempfile::NamedTempFile::new_in(dir)?;
    let summary_file = tempfile::NamedTempFile::new_in(dir)?;
    crate::artifact::write_json_pretty(
        golden_file.path(),
        &json!({
            "schema_version": 2,
            "dimension": "snippet",
            "git_head": git_head_text,
            "windows": golden,
            "oracle_note": "oracle_window is computed from the fixed ingested source and needle, independently of observed_snippet; any mismatch fails the source-window gate",
        }),
    )?;
    summary.write_to(summary_file.path())?;
    golden_file.as_file().sync_all()?;
    summary_file.as_file().sync_all()?;
    let _golden = golden_file.persist(dir.join("golden_windows.json"))?;
    let _summary = summary_file.persist(dir.join("summary.json"))?;
    Ok(())
}

/// Remove an earlier verdict before a new direct producer attempt can fail.
/// The summary is the commit marker, so remove it before its supplemental gold.
pub fn invalidate_artifacts(dir: &Path) -> std::io::Result<()> {
    for name in ["summary.json", "golden_windows.json"] {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index into failure vectors and slices whose shape this module produces and asserts directly; an out-of-range index is a legitimate test failure"
)]
mod tests {
    use super::*;
    use quanta_index_contract::{LqExpr, LqLeaf, parse_native_content_query};

    fn publication_fixture_report() -> SnippetReport {
        let scores = SNIPPET_QUERIES
            .iter()
            .map(|query| SnippetScore {
                id: query.id,
                intent: query.intent,
                query: query.query,
                needle: query.needle,
                expect_path: query.expect_path,
                matched_path: None,
                snippet: None,
                metrics: None,
                observed_hit_offset: None,
                observed_highlights: Vec::new(),
                ranked_hits: None,
                matched_total: None,
                failures: vec!["writer fixture has no observation".into()],
            })
            .collect();
        SnippetReport {
            scores,
            passed: false,
            native_provenance: None,
        }
    }

    #[test]
    fn artifact_writer_replaces_a_prior_pair_with_complete_current_rows() -> AnyResult<()> {
        let output = tempfile::tempdir()?;
        let head = GitHeadV1::parse(&"a".repeat(40))?;
        std::fs::write(output.path().join("summary.json"), b"old green")?;
        std::fs::write(output.path().join("golden_windows.json"), b"old gold")?;
        write_artifacts(
            &publication_fixture_report(),
            output.path(),
            head,
            HostV1::observe()?,
        )?;
        let summary: Value =
            serde_json::from_slice(&std::fs::read(output.path().join("summary.json"))?)?;
        let golden: Value =
            serde_json::from_slice(&std::fs::read(output.path().join("golden_windows.json"))?)?;
        assert_eq!(summary["schema_version"], 2);
        assert_eq!(summary["provenance"]["git_head"], "a".repeat(40));
        assert_eq!(golden["git_head"], summary["provenance"]["git_head"]);
        assert_eq!(summary["rows"].as_array().map(Vec::len), Some(4));
        assert_eq!(golden["windows"].as_array().map(Vec::len), Some(4));
        assert_eq!(summary["detail"]["passed"], false);
        Ok(())
    }

    #[test]
    fn artifact_writer_removes_prior_green_before_failed_gold_publish() -> AnyResult<()> {
        let output = tempfile::tempdir()?;
        std::fs::write(
            output.path().join("summary.json"),
            br#"{"detail":{"passed":true}}"#,
        )?;
        std::fs::create_dir(output.path().join("golden_windows.json"))?;
        let result = write_artifacts(
            &publication_fixture_report(),
            output.path(),
            GitHeadV1::parse(&"b".repeat(40))?,
            HostV1::observe()?,
        );
        assert!(
            result.is_err(),
            "golden path directory must refuse publication"
        );
        assert!(
            !output.path().join("summary.json").exists(),
            "a failed new run must not leave a previous green summary"
        );
        Ok(())
    }

    #[test]
    fn judged_phrase_and_regex_use_admitted_native_content_syntax() {
        let phrase = parse_native_content_query(SNIPPET_QUERIES[0].query)
            .expect("phrase fixture must parse");
        assert!(matches!(
            &phrase.expr,
            LqExpr::Leaf(LqLeaf::Phrase(text)) if text == "brown fox marker"
        ));
        let regex =
            parse_native_content_query(SNIPPET_QUERIES[1].query).expect("regex fixture must parse");
        assert!(matches!(
            &regex.expr,
            LqExpr::All(children) if children.iter().any(|expr| matches!(expr,
                LqExpr::Leaf(LqLeaf::Keyword(text)) if text == "regex_target_symbol"
            )) && children.iter().any(|expr| matches!(expr,
                LqExpr::Leaf(LqLeaf::Regex(source)) if source == r"regex_target_symbol\(input: &str\)"
            ))
        ));
        assert!(
            quanta_index_lexical::native::validate_native_text_input(&phrase)
                .expect("phrase fixture must be admitted by Native")
                > 0
        );
        assert!(
            quanta_index_lexical::native::validate_native_text_input(&regex)
                .expect("anchored regex fixture must be admitted by Native")
                > 0
        );
    }

    #[test]
    fn decoys_expose_dropped_predicates_without_changing_judged_rows() {
        assert_eq!(SNIPPET_QUERIES.len(), 4);
        let phrase = "brown fox marker";
        let phrase_decoy = fixture_source("src/snippet/phrase_decoy.rs").expect("phrase decoy");
        assert!(
            phrase
                .split_ascii_whitespace()
                .all(|term| phrase_decoy.contains(term))
        );
        assert!(!phrase_decoy.contains(phrase));
        let expected = fixture_source("src/snippet/regex.rs").expect("regex source");
        let decoy = fixture_source("src/snippet/regex_decoy.rs").expect("regex decoy");
        assert!(expected.contains("regex_target_symbol"));
        assert!(decoy.contains("regex_target_symbol"));
        assert!(expected.contains("regex_target_symbol(input: &str)"));
        assert!(!decoy.contains("regex_target_symbol(input: &str)"));
        for intent in [SnippetIntent::Phrase, SnippetIntent::Regex] {
            assert!(predicate_population_failures(intent, 1).is_empty());
            assert!(!predicate_population_failures(intent, 2).is_empty());
        }
    }

    // --- pure window oracle ---------------------------------------------

    #[test]
    fn short_source_is_emitted_whole() {
        let src = "fn f() { needle_here(); }";
        let w = compute_snippet_window(src, "needle_here", MAX_SNIPPET_LEN).expect("hit");
        assert_eq!(w.text, src, "short source must not be truncated");
        assert_eq!(w.hit_offset, src.find("needle_here").expect("present"));
    }

    #[test]
    fn absent_needle_yields_no_window() {
        assert!(compute_snippet_window("abcdef", "zzz", 64).is_none());
        assert!(compute_snippet_window("abc", "", 64).is_none());
    }

    #[test]
    fn long_source_truncates_and_centers() {
        let prefix = "a".repeat(500);
        let suffix = "b".repeat(500);
        let src = format!("{prefix}NEEDLE{suffix}");
        let w = compute_snippet_window(&src, "NEEDLE", 100).expect("hit");
        assert!(w.text.len() <= 100, "window over budget: {}", w.text.len());
        assert!(w.text.contains("NEEDLE"), "window dropped the needle");
        // Centered: leading context retained, so the hit is not at offset 0.
        assert!(
            w.hit_offset >= MIN_LEADING_CONTEXT,
            "hit pinned to leading edge at offset {}",
            w.hit_offset
        );
        // ...and trailing context retained too.
        let tail = w.hit_offset.saturating_add("NEEDLE".len());
        assert!(tail < w.text.len(), "no trailing context retained");
    }

    #[test]
    fn window_is_deterministic() {
        let src = format!("{}TARGET{}", "x".repeat(400), "y".repeat(400));
        let a = compute_snippet_window(&src, "TARGET", 120).expect("hit");
        let b = compute_snippet_window(&src, "TARGET", 120).expect("hit");
        assert_eq!(a, b, "window must be byte-identical across runs");
    }

    #[test]
    fn window_clamps_to_char_boundary() {
        // Multibyte chars surrounding the hit; window must stay valid UTF-8.
        let src = format!("{}NEEDLE{}", "é".repeat(200), "ü".repeat(200));
        let w = compute_snippet_window(&src, "NEEDLE", 80).expect("hit");
        // If text built, it is valid UTF-8 by construction (String); assert hit.
        assert!(w.text.contains("NEEDLE"));
        assert!(w.text.len() <= 80);
    }

    #[test]
    fn window_fits_the_whole_needle_before_assigning_context() {
        let source = "abcéABCDEzzzzzz";
        let window = compute_snippet_window(source, "ABCDE", 6).expect("fitting hit");
        assert_eq!(window.text, "ABCDEz");
        assert_eq!(window.hit_offset, 0);
        assert!(compute_snippet_window(source, "ABCDE", 4).is_none());
    }

    // --- observed metrics ------------------------------------------------

    #[test]
    fn measure_counts_multiple_hits() {
        let m = measure_snippet("foo bar foo baz foo", "foo");
        assert!(m.needle_present);
        assert_eq!(m.multi_hit_count, 3);
        assert_eq!(m.hit_offset_within_window, Some(0));
    }

    #[test]
    fn measure_reports_absent_needle() {
        let m = measure_snippet("nothing here", "missing");
        assert!(!m.needle_present);
        assert_eq!(m.multi_hit_count, 0);
        assert_eq!(m.hit_offset_within_window, None);
        assert_eq!(m.center_ratio, None);
    }

    #[test]
    fn measure_center_ratio_midpoint() {
        // hit at offset 4 in a 10-byte snippet -> ratio 0.4.
        let m = measure_snippet("abcdNEEDLEx".get(0..10).unwrap_or("abcdNEEDLEx"), "NEEDLE");
        assert!(m.needle_present);
        let ratio = m.center_ratio.expect("ratio");
        assert!((ratio - 0.4).abs() < 1e-9, "ratio {ratio}");
    }

    // --- gate (must be able to go RED) -----------------------------------

    #[test]
    fn gate_passes_well_centered_long_line() {
        // 100-byte window, hit at offset 40 -> centered, bounded, present.
        let snippet = format!("{}NEEDLE{}", "a".repeat(40), "b".repeat(40));
        let m = measure_snippet(&snippet, "NEEDLE");
        let failures = gate_snippet(
            SnippetIntent::LongLine,
            &m,
            MAX_SNIPPET_LEN,
            MIN_LEADING_CONTEXT,
        );
        assert!(failures.is_empty(), "unexpected failures: {failures:?}");
    }

    #[test]
    fn gate_trips_on_absent_needle() {
        let m = measure_snippet("unrelated content", "compute_target");
        let failures = gate_snippet(
            SnippetIntent::Phrase,
            &m,
            MAX_SNIPPET_LEN,
            MIN_LEADING_CONTEXT,
        );
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("needle absent"), "{failures:?}");
    }

    #[test]
    fn gate_trips_on_unbounded_long_line() {
        // Synthetic full untruncated long line: needle present but length over
        // bound and pinned to the leading edge -> the honest RED an unwindowed
        // snippet would produce, proving the gate trips (the live engine no longer
        // emits this shape; it truncates to a hit-centered window).
        let snippet = format!("NEEDLE{}", "z".repeat(MAX_SNIPPET_LEN + 50));
        let m = measure_snippet(&snippet, "NEEDLE");
        let failures = gate_snippet(
            SnippetIntent::LongLine,
            &m,
            MAX_SNIPPET_LEN,
            MIN_LEADING_CONTEXT,
        );
        assert!(
            failures.iter().any(|f| f.contains("exceeds bound")),
            "expected bound violation, got {failures:?}"
        );
        assert!(
            failures.iter().any(|f| f.contains("leading edge")),
            "expected centering violation, got {failures:?}"
        );
    }

    #[test]
    fn gate_trips_on_single_hit_multi_intent() {
        let m = measure_snippet("only one marker here", "marker");
        let failures = gate_snippet(
            SnippetIntent::MultiHit,
            &m,
            MAX_SNIPPET_LEN,
            MIN_LEADING_CONTEXT,
        );
        assert!(
            failures.iter().any(|f| f.contains("multiple needle")),
            "{failures:?}"
        );
    }

    #[test]
    fn gate_exempts_short_phrase_from_centering() {
        // A short phrase snippet with the hit near the start is fine: no
        // truncation was required, so no centering obligation.
        let m = measure_snippet("brown fox marker in a short line", "brown fox marker");
        let failures = gate_snippet(
            SnippetIntent::Phrase,
            &m,
            MAX_SNIPPET_LEN,
            MIN_LEADING_CONTEXT,
        );
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn fixed_source_gate_rejects_orphan_context_with_valid_metrics() {
        let query = &SNIPPET_QUERIES[0];
        let orphan = "unrelated brown fox marker context";
        let metrics = measure_snippet(orphan, query.needle);
        assert!(
            gate_snippet(query.intent, &metrics, MAX_SNIPPET_LEN, MIN_LEADING_CONTEXT).is_empty(),
            "the old metric gate alone must accept this counterexample"
        );
        assert_eq!(
            fixture_window_failures(query, orphan, None, &[]),
            vec![format!(
                "fixed source window differs from emitted snippet for `{}`",
                query.expect_path
            )]
        );
        let score = SnippetScore {
            id: query.id,
            intent: query.intent,
            query: query.query,
            needle: query.needle,
            expect_path: query.expect_path,
            matched_path: Some(query.expect_path.to_string()),
            snippet: Some(orphan.to_string()),
            metrics: Some(metrics),
            observed_hit_offset: None,
            observed_highlights: Vec::new(),
            ranked_hits: Some(1),
            matched_total: Some(1),
            failures: fixture_window_failures(query, orphan, None, &[]),
        };
        let golden = golden_window_json(&score);
        assert_ne!(golden["observed_snippet"], golden["oracle_window"]);
        assert_eq!(
            golden["oracle_window"],
            fixture_source(query.expect_path).expect("fixed source")
        );
        assert!(!score.passed());
    }

    #[test]
    fn fixed_source_gate_requires_emitted_anchor_and_every_highlight() {
        let query = &SNIPPET_QUERIES[2];
        let observed = fixture_source(query.expect_path).expect("fixed multi-hit source");
        let spans = [
            HighlightSpan { start: 3, len: 17 },
            HighlightSpan { start: 25, len: 17 },
            HighlightSpan { start: 46, len: 17 },
        ];
        assert!(fixture_window_failures(query, observed, Some(3), &spans).is_empty());
        assert_eq!(
            fixture_window_failures(query, observed, None, &spans[..1]),
            [
                format!(
                    "fixed source hit offset differs for `{}`",
                    query.expect_path
                ),
                format!("fixed source highlights differ for `{}`", query.expect_path),
            ]
        );
    }

    #[test]
    fn fixed_source_oracle_survives_missing_observation() {
        let query = &SNIPPET_QUERIES[3];
        let score = SnippetScore {
            id: query.id,
            intent: query.intent,
            query: query.query,
            needle: query.needle,
            expect_path: query.expect_path,
            matched_path: None,
            snippet: None,
            metrics: None,
            observed_hit_offset: None,
            observed_highlights: Vec::new(),
            ranked_hits: None,
            matched_total: None,
            failures: vec!["retrieval miss".into()],
        };
        let golden = golden_window_json(&score);
        assert!(golden["observed_snippet"].is_null());
        assert_eq!(
            golden["oracle_window"],
            fixture_window(query.expect_path, query.needle)
                .expect("fixed long-line source")
                .text
        );
    }
}
