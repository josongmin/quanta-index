//! UI/UX contract rail (J7Q-07): prove the typed UI highlight-anchor field.
//!
//! J7Q-07 adds `LexicalCandidate::snippet_hit_offset` so a downstream UI can
//! place a highlight on the matched hit WITHOUT regex-parsing the raw snippet to
//! re-find the term. This rail proves the typed field actually carries a usable
//! anchor end to end: it seeds fixtures (a short hit and a long-line hit), runs
//! the live ranker, and for each checks two blocking properties:
//!
//! 1. **anchor present** — the served candidate carries `Some(offset)` (the typed
//!    field is populated, not left implicit in the snippet text);
//! 2. **anchor correct** — `snippet[offset..]` begins with the matched needle, so
//!    a consumer that slices at the offset lands exactly on the hit (no semantic
//!    guesswork, no re-derivation from raw text).
//!
//! It also proves the **explanation sections** are consumer-renderable: a served
//! candidate is run back through the `explain` route under the query that
//! ranked it, and the returned `SearchExplanation` is graded for TYPED,
//! route-specific sectioned provenance — the planner-trace stages (`plan`,
//! `merge`, …), the engines touched, the strategy tag, a typed presence, and
//! the contribution rows whose sum is the candidate's carried score
//! (QI-BB-022) — so a UI can render the explanation section by section without
//! regex-parsing the free-form `summary`. The assertion is route-specific (the
//! exact stages / engine / strategy / rows the explain route emits), not merely
//! "non-empty".
//!
//! Fail-closed posture: a missing anchor, an out-of-range offset, an offset that
//! does not point at the needle, or an explanation missing its typed sections is
//! a rail failure recorded as-is — the rail never fabricates an anchor or a
//! section to make the gate pass. It grades the emitted contract as-is, proving
//! the new typed fields across the engine + contract layers (not a single
//! layer).

use std::path::Path;

use anyhow::Result as AnyResult;
use quanta_index_contract::{CandidatePresenceV1, EarlyStopReason, HighlightSpan, TextQuerySyntax};
use serde_json::{Value, json};

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest, corpus_digest,
    saturating_u64,
};
use crate::harness::E2eRuntime;

/// Repo id the UI fixtures are ingested under.
pub const UI_REPO: &str = "repo-ui";

/// Result cap requested for every UI probe query.
pub const TOP_K: u32 = 10;

/// A long source line whose needle is buried near the middle, so the engine must
/// truncate to a hit-centered window and the reported offset is *within* that
/// window (not the raw-source position).
const UI_LONG_LINE: &str = "let lead_padding_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa = ui_anchor_marker_zzz(0) + tail_padding_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb;\n";

/// One seeded UI probe: a fixture document plus the query + needle the typed
/// anchor must point at.
#[derive(Clone, Copy, Debug)]
pub struct UiProbe {
    /// Stable probe id (artifact key + regression anchor).
    pub id: &'static str,
    /// Repo-relative fixture path.
    pub path: &'static str,
    /// Fixture content.
    pub content: &'static str,
    /// Query text issued to the engine.
    pub query: &'static str,
    /// The literal substring the anchor must point at.
    pub needle: &'static str,
}

/// The judged UI probe set (the UI-contract rail SSOT).
pub const UI_PROBES: &[UiProbe] = &[
    UiProbe {
        id: "ui.short.symbol",
        path: "src/ui/short.rs",
        content: "fn header() {}\npub fn ui_short_marker(input: &str) -> usize { input.len() }\nfn footer() {}\n",
        query: "ui_short_marker",
        needle: "ui_short_marker",
    },
    UiProbe {
        id: "ui.longline.anchor",
        path: "src/ui/longline.rs",
        content: UI_LONG_LINE,
        query: "ui_anchor_marker_zzz",
        needle: "ui_anchor_marker_zzz",
    },
    UiProbe {
        id: "ui.multihit.spans",
        path: "src/ui/multi.rs",
        content: "fn ui_multi_marker() { ui_multi_marker(); ui_multi_marker(); }\n",
        query: "ui_multi_marker",
        needle: "ui_multi_marker",
    },
];

/// `u32` snippet offset to `usize` for slicing.
///
/// A `u32` value always fits `usize` on every supported (>= 32-bit) target, so
/// the widening is exact.
#[expect(
    clippy::as_conversions,
    reason = "u32 snippet offset always fits usize on supported (>= 32-bit) targets"
)]
fn usize_from_offset(offset: u32) -> usize {
    offset as usize
}

/// Scored outcome for one UI probe.
#[derive(Clone, Debug)]
pub struct UiScore {
    pub id: &'static str,
    pub query: &'static str,
    pub needle: &'static str,
    pub path: &'static str,
    /// The exact emitted snippet (the consumer-visible text).
    pub snippet: Option<String>,
    /// The typed primary highlight anchor the candidate carried.
    pub snippet_hit_offset: Option<u32>,
    /// Every typed highlight span the candidate carried.
    pub highlights: Vec<HighlightSpan>,
    pub failures: Vec<String>,
}

impl UiScore {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// The typed explanation sections captured for one served candidate.
///
/// These are the consumer-facing "sections" of a `SearchExplanation`: a UI maps
/// each typed field to a rendered section (planner stages → "Planning", engines →
/// "Engines", strategy → "Strategy", contributions → "Score") with no regex
/// parsing of the free-form summary. `contributions_count` gates: a scored
/// explain carries one lexical row whose sum is the carried score (QI-BB-022).
#[derive(Clone, Debug)]
pub struct ExplanationSectionsCapture {
    pub probe_path: &'static str,
    /// Typed planner-trace stage tags, in emission order (e.g. `plan`, `merge`).
    pub planner_stages: Vec<&'static str>,
    /// Typed engine identifiers the route touched (e.g. `lexical`).
    pub engines_touched: Vec<&'static str>,
    /// The route's strategy tag.
    pub strategy: String,
    /// Ranking-contribution row count (rerank-surface owned; informational here).
    pub contributions_count: usize,
    /// Whether the route emitted a non-empty human summary alongside the sections.
    pub has_summary: bool,
    /// Typed early-stop reason tag, if the route stopped early.
    pub early_stop_reason: Option<&'static str>,
    pub failures: Vec<String>,
}

impl ExplanationSectionsCapture {
    /// A fail-closed capture: empty sections plus the recorded failure(s). Used
    /// when the candidate could not be retrieved or explained at all.
    fn failed(probe_path: &'static str, failures: Vec<String>) -> Self {
        Self {
            probe_path,
            planner_stages: Vec::new(),
            engines_touched: Vec::new(),
            strategy: String::new(),
            contributions_count: 0,
            has_summary: false,
            early_stop_reason: None,
            failures,
        }
    }

    #[must_use]
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// The full UI-contract report.
#[derive(Clone, Debug)]
pub struct UiReport {
    pub scores: Vec<UiScore>,
    pub explanation: ExplanationSectionsCapture,
    pub passed: bool,
}

/// Boot one runtime, seed the UI fixtures, seal + activate.
pub fn prepare_ui_runtime() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    for probe in UI_PROBES {
        rt.ingest_text(UI_REPO, probe.path, probe.content)?;
    }
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

/// Score one UI probe end to end against the live ranker.
fn score_probe(rt: &mut E2eRuntime, probe: &UiProbe) -> UiScore {
    let mut failures = Vec::new();
    let result = rt.query_text(TextQuerySyntax::Native, probe.query, TOP_K);
    if let Some(error) = result.typed_error {
        failures.push(format!(
            "{}: query returned typed error {}: {}",
            probe.id, error.code, error.message
        ));
        return UiScore {
            id: probe.id,
            query: probe.query,
            needle: probe.needle,
            path: probe.path,
            snippet: None,
            snippet_hit_offset: None,
            highlights: Vec::new(),
            failures,
        };
    }
    let Some(candidate) = result
        .candidates
        .iter()
        .find(|candidate| candidate.repo_relative_path.as_str() == probe.path)
    else {
        failures.push(format!(
            "{}: expected candidate for `{}` not retrieved (anchor ungradable)",
            probe.id, probe.path
        ));
        return UiScore {
            id: probe.id,
            query: probe.query,
            needle: probe.needle,
            path: probe.path,
            snippet: None,
            snippet_hit_offset: None,
            highlights: Vec::new(),
            failures,
        };
    };
    let snippet = candidate.snippet.clone();
    let snippet_hit_offset = candidate.snippet_hit_offset;
    let highlights = candidate.highlights.clone();
    // 1. Anchor present — the typed field must be populated, not implicit.
    match snippet_hit_offset {
        None => failures.push(format!(
            "{}: candidate carries no typed snippet_hit_offset (consumer must re-find the hit)",
            probe.id
        )),
        // 2. Anchor correct — slicing the snippet at the offset lands on the needle.
        Some(offset) => {
            let at = usize_from_offset(offset);
            match snippet.get(at..) {
                Some(tail) if tail.starts_with(probe.needle) => {}
                Some(_) => failures.push(format!(
                    "{}: snippet_hit_offset {offset} does not point at `{}` (anchor wrong)",
                    probe.id, probe.needle
                )),
                None => failures.push(format!(
                    "{}: snippet_hit_offset {offset} is out of range / not a char boundary for the {}-byte snippet",
                    probe.id,
                    snippet.len()
                )),
            }
        }
    }
    // 3. Highlight spans present — the typed span set must not be empty when the
    //    needle is in the emitted snippet.
    let expected_hits = snippet.matches(probe.needle).count();
    if expected_hits > 0 && highlights.is_empty() {
        failures.push(format!(
            "{}: snippet contains the needle but carries no highlight spans (consumer must re-find)",
            probe.id
        ));
    }
    // 4. Every span correct — each span covers the needle exactly.
    for span in &highlights {
        let start = usize_from_offset(span.start);
        let end = start.saturating_add(usize_from_offset(span.len));
        match snippet.get(start..end) {
            Some(text) if text == probe.needle => {}
            _ => failures.push(format!(
                "{}: highlight span (start {}, len {}) does not cover `{}` exactly",
                probe.id, span.start, span.len, probe.needle
            )),
        }
    }
    // 5. Multi-hit coverage — the span set covers every needle occurrence.
    if highlights.len() != expected_hits {
        failures.push(format!(
            "{}: {} highlight spans for {} needle occurrences in the snippet",
            probe.id,
            highlights.len(),
            expected_hits
        ));
    }
    UiScore {
        id: probe.id,
        query: probe.query,
        needle: probe.needle,
        path: probe.path,
        snippet: Some(snippet),
        snippet_hit_offset,
        highlights,
        failures,
    }
}

/// Capture and grade the typed explanation sections for one probe's candidate.
///
/// Runs the served candidate back through the `explain` route under the query
/// that ranked it and grades the returned explanation for route-specific TYPED
/// sections (not just non-empty): the explain route must surface the `plan`
/// and `merge` planner stages, touch exactly the lexical engine, carry the
/// `lexical_score_trace` strategy tag, report the candidate as indexed, emit
/// contribution rows summing to the candidate's carried score, and emit a
/// non-empty summary. Anything missing is a fail-closed rail failure.
fn capture_explanation_sections(
    rt: &mut E2eRuntime,
    probe: &UiProbe,
) -> ExplanationSectionsCapture {
    let result = rt.query_text(TextQuerySyntax::Native, probe.query, TOP_K);
    let candidate = result
        .candidates
        .iter()
        .find(|candidate| candidate.repo_relative_path.as_str() == probe.path)
        .cloned();
    let Some(candidate) = candidate else {
        return ExplanationSectionsCapture::failed(
            probe.path,
            vec![format!(
                "explanation: no candidate for `{}` to explain (sections ungradable)",
                probe.path
            )],
        );
    };
    let carried_score = candidate.score;
    let explain = rt.explain_candidate_under_query(candidate, TextQuerySyntax::Native, probe.query);
    if let Some(error) = explain.typed_error {
        return ExplanationSectionsCapture::failed(
            probe.path,
            vec![format!(
                "explanation: explain route returned typed error {}: {}",
                error.code, error.message
            )],
        );
    }
    let Some(explanation) = explain.explanation else {
        return ExplanationSectionsCapture::failed(
            probe.path,
            vec!["explanation: explain route produced no explanation".to_string()],
        );
    };
    let planner_stages: Vec<&'static str> = explanation
        .planner_trace
        .iter()
        .map(|entry| entry.stage.as_str())
        .collect();
    let engines_touched: Vec<&'static str> = explanation
        .engines_touched
        .iter()
        .map(|engine| engine.as_str())
        .collect();
    let early_stop_reason = explanation.early_stop_reason.map(EarlyStopReason::as_str);
    let strategy = explanation.strategy;
    let contributions_count = explanation.contributions.len();
    let contribution_sum: f32 = explanation
        .contributions
        .iter()
        .map(|row| row.contribution)
        .sum();
    let has_summary = !explanation.summary.is_empty();
    // Route-specific section assertions — a consumer renders these typed sections
    // without parsing the summary string, so each must be present and correct.
    let mut failures = Vec::new();
    if !planner_stages.contains(&"plan") {
        failures.push(format!(
            "explanation: planner sections missing the `plan` stage (got {planner_stages:?})"
        ));
    }
    if !planner_stages.contains(&"merge") {
        failures.push(format!(
            "explanation: planner sections missing the `merge` stage (got {planner_stages:?})"
        ));
    }
    if engines_touched != ["lexical"] {
        failures.push(format!(
            "explanation: explain route must touch exactly the lexical engine (got {engines_touched:?})"
        ));
    }
    if strategy != "lexical_score_trace" {
        failures.push(format!(
            "explanation: explain route strategy section must be `lexical_score_trace` (got `{strategy}`)"
        ));
    }
    if explain.presence != Some(CandidatePresenceV1::Indexed) {
        failures.push(format!(
            "explanation: a served candidate must explain as indexed (got {:?})",
            explain.presence
        ));
    }
    if contributions_count == 0 {
        failures.push("explanation: a scored explain carries no contribution rows".to_string());
    } else if (contribution_sum - carried_score).abs() > 1e-5 * carried_score.abs().max(1.0) {
        failures.push(format!(
            "explanation: contribution rows sum to {contribution_sum} but the candidate carried {carried_score}"
        ));
    }
    if !has_summary {
        failures.push("explanation: summary section is empty".to_string());
    }
    ExplanationSectionsCapture {
        probe_path: probe.path,
        planner_stages,
        engines_touched,
        strategy,
        contributions_count,
        has_summary,
        early_stop_reason,
        failures,
    }
}

/// Run the full UI-contract rail against a freshly seeded runtime.
pub fn run_ui_report() -> AnyResult<UiReport> {
    let mut rt = prepare_ui_runtime()?;
    let mut scores = Vec::with_capacity(UI_PROBES.len());
    for probe in UI_PROBES {
        scores.push(score_probe(&mut rt, probe));
    }
    // Explanation sections: prove the typed sectioned provenance is consumer-
    // renderable. Use the first (deterministic short-symbol) probe's candidate.
    let explanation = UI_PROBES.first().map_or_else(
        || {
            ExplanationSectionsCapture::failed(
                "<none>",
                vec!["explanation: UI_PROBES is empty".to_string()],
            )
        },
        |probe| capture_explanation_sections(&mut rt, probe),
    );
    let passed = scores.iter().all(UiScore::passed) && explanation.passed();
    Ok(UiReport {
        scores,
        explanation,
        passed,
    })
}

// ---------------------------------------------------------------------------
// Artifact emission.
// ---------------------------------------------------------------------------

fn score_json(score: &UiScore) -> Value {
    json!({
        "id": score.id,
        "query": score.query,
        "needle": score.needle,
        "path": score.path,
        "snippet": score.snippet,
        "snippet_hit_offset": score.snippet_hit_offset,
        "highlights": score
            .highlights
            .iter()
            .map(|span| json!({ "start": span.start, "len": span.len }))
            .collect::<Vec<_>>(),
        "failures": score.failures,
        "passed": score.passed(),
    })
}

/// The typed explanation-sections snapshot for the report's graded candidate.
fn explanation_sections_json(capture: &ExplanationSectionsCapture) -> Value {
    json!({
        "probe_path": capture.probe_path,
        "planner_stages": capture.planner_stages,
        "engines_touched": capture.engines_touched,
        "strategy": capture.strategy,
        "contributions_count": capture.contributions_count,
        "has_summary": capture.has_summary,
        "early_stop_reason": capture.early_stop_reason,
        "failures": capture.failures,
        "passed": capture.passed(),
    })
}

/// The UI contract snapshot supplement: never an authority artifact.
fn contract_snapshots_supplement_json(report: &UiReport, git_head: &GitHeadV1) -> Value {
    json!({
        "kind": "quanta-index-benchmark-supplement",
        "supplement_schema_version": 1,
        "dimension": "ui",
        "git_head": git_head.as_str(),
        "fields_under_test": ["snippet", "snippet_hit_offset", "highlights", "explanation_sections"],
        "snapshot_note": "snippet_hit_offset + highlights are the typed UI anchors (J7Q-07); a consumer slices snippet at offset/spans to land on hits, with no regex parsing of raw text. explanation_sections exposes the typed planner stages / engines / strategy a consumer renders as sections without parsing the summary string",
        "snapshots": report.scores.iter().map(score_json).collect::<Vec<_>>(),
        "explanation_sections": explanation_sections_json(&report.explanation),
    })
}

/// The current-source authority envelope for consumer-facing UI contracts.
pub fn artifact(
    report: &UiReport,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    let corpus = UI_PROBES
        .iter()
        .map(|probe| (probe.path.to_string(), probe.content.to_string()))
        .collect::<Vec<_>>();
    let mut rows = report
        .scores
        .iter()
        .map(|score| BenchRowV1 {
            scenario_id: score.id.to_string(),
            route_family: RouteFamily::Lexical,
            syntax: BenchSyntax::Native,
            result_shape: if score.snippet.is_some() {
                ResultShape::Candidates
            } else {
                ResultShape::Empty
            },
            latency: None,
            qps: None,
            error_count: saturating_u64(score.failures.len()),
            timeout_count: 0,
            result_count: Some(if score.snippet.is_some() { 1 } else { 0 }),
            typed_error_code: None,
            engine_touched: vec!["lexical".to_string()],
            early_stop_reason: None,
        })
        .collect::<Vec<_>>();
    rows.push(BenchRowV1 {
        scenario_id: "ui.explanation_sections".to_string(),
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        result_shape: if report.explanation.passed() {
            ResultShape::Candidates
        } else {
            ResultShape::Empty
        },
        latency: None,
        qps: None,
        error_count: saturating_u64(report.explanation.failures.len()),
        timeout_count: 0,
        result_count: Some(if report.explanation.passed() { 1 } else { 0 }),
        typed_error_code: None,
        engine_touched: vec!["lexical".to_string()],
        early_stop_reason: None,
    });
    Ok(BenchArtifactV1 {
        dimension: "ui".to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest("ui", &corpus),
            config_digest: config_digest(
                "ui",
                &[
                    ("probes", UI_PROBES.len().to_string()),
                    ("top_k", TOP_K.to_string()),
                ],
            ),
            model_revision: None,
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows,
        detail: json!({
            "passed": report.passed,
            "blocking_signal": "every served candidate must carry a typed snippet_hit_offset that points exactly at the matched needle, and explain must surface route-specific typed sections",
            "scores": report.scores.iter().map(score_json).collect::<Vec<_>>(),
            "explanation_sections": explanation_sections_json(&report.explanation),
        }),
    })
}

/// Write the two canonical UI artifacts under `dir`:
/// `summary.json` and `contract_snapshots.json`.
pub fn write_artifacts(
    report: &UiReport,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<()> {
    artifact(report, git_head.clone(), host)?.write_to(&dir.join("summary.json"))?;
    crate::artifact::write_json_pretty(
        &dir.join("contract_snapshots.json"),
        &contract_snapshots_supplement_json(report, &git_head),
    )?;
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index JSON values whose shape this module constructs and asserts directly; an out-of-range index is a legitimate test failure"
)]
mod tests {
    use super::*;

    #[test]
    fn probe_set_is_non_empty_and_well_formed() {
        assert!(!UI_PROBES.is_empty(), "UI rail needs at least one probe");
        for probe in UI_PROBES {
            assert!(
                probe.content.contains(probe.needle),
                "probe {} fixture must contain its needle",
                probe.id
            );
        }
    }

    #[test]
    fn summary_json_records_passed_and_scores() {
        let report = UiReport {
            scores: vec![UiScore {
                id: "ui.test",
                query: "q",
                needle: "marker",
                path: "src/x.rs",
                snippet: Some("a marker b".to_string()),
                snippet_hit_offset: Some(2),
                highlights: vec![HighlightSpan { start: 2, len: 6 }],
                failures: Vec::new(),
            }],
            explanation: ExplanationSectionsCapture {
                probe_path: "src/x.rs",
                planner_stages: vec!["plan", "merge"],
                engines_touched: vec!["lexical"],
                strategy: "lexical_score_trace".to_string(),
                contributions_count: 1,
                has_summary: true,
                early_stop_reason: None,
                failures: Vec::new(),
            },
            passed: true,
        };
        assert!(report.passed);
        let head = GitHeadV1::parse(&"a".repeat(40)).expect("head");
        let value = contract_snapshots_supplement_json(&report, &head);
        assert_eq!(value["dimension"], "ui");
        assert_eq!(value["snapshots"][0]["snippet_hit_offset"], 2);
        assert_eq!(value["snapshots"][0]["highlights"][0]["start"], 2);
        assert_eq!(value["snapshots"][0]["highlights"][0]["len"], 6);
        assert_eq!(value["explanation_sections"]["planner_stages"][0], "plan");
        assert_eq!(
            value["explanation_sections"]["strategy"],
            "lexical_score_trace"
        );
        let head = GitHeadV1::parse(&"a".repeat(40)).expect("head");
        let snaps = contract_snapshots_supplement_json(&report, &head);
        assert_eq!(snaps["fields_under_test"][1], "snippet_hit_offset");
        assert_eq!(snaps["fields_under_test"][2], "highlights");
        assert_eq!(snaps["fields_under_test"][3], "explanation_sections");
        assert_eq!(
            snaps["explanation_sections"]["engines_touched"][0],
            "lexical"
        );
        assert_eq!(snaps["explanation_sections"]["passed"], true);
        let artifact = artifact(
            &report,
            GitHeadV1::parse(&"a".repeat(40)).expect("head"),
            HostV1::observe().expect("host"),
        )
        .expect("authority artifact")
        .to_json()
        .expect("json");
        assert_eq!(artifact["schema_version"], 2);
        assert_eq!(artifact["detail"]["passed"], true);
        assert_eq!(artifact["rows"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn seeded_ui_rail_runs_and_anchors_every_probe() {
        let report = run_ui_report().expect("ui rail runs");
        assert_eq!(
            report.scores.len(),
            UI_PROBES.len(),
            "every probe must be scored"
        );
        for score in &report.scores {
            assert!(
                score.passed(),
                "probe {} failed UI anchor checks: {:?}",
                score.id,
                score.failures
            );
            let offset = score
                .snippet_hit_offset
                .unwrap_or_else(|| panic!("{}: no typed anchor", score.id));
            let snippet = score
                .snippet
                .as_ref()
                .unwrap_or_else(|| panic!("{}: no snippet", score.id));
            let at = usize_from_offset(offset);
            assert!(
                snippet
                    .get(at..)
                    .is_some_and(|tail| tail.starts_with(score.needle)),
                "probe {}: offset {offset} must point at `{}` in snippet {snippet:?}",
                score.id,
                score.needle
            );
            // Every highlight span must cover the needle exactly, and the span
            // set must cover every occurrence (multi-hit coverage).
            assert!(
                !score.highlights.is_empty(),
                "{}: no highlight spans",
                score.id
            );
            for span in &score.highlights {
                let start = usize_from_offset(span.start);
                let end = start.saturating_add(usize_from_offset(span.len));
                assert_eq!(
                    snippet.get(start..end),
                    Some(score.needle),
                    "probe {}: span (start {}, len {}) must cover the needle exactly",
                    score.id,
                    span.start,
                    span.len
                );
            }
            assert_eq!(
                score.highlights.len(),
                snippet.matches(score.needle).count(),
                "probe {}: highlight spans must cover every needle occurrence",
                score.id
            );
        }
        // The explain route must surface its typed sections route-specifically.
        let explanation = &report.explanation;
        assert!(
            explanation.passed(),
            "explanation sections failed: {:?}",
            explanation.failures
        );
        assert!(
            explanation.planner_stages.contains(&"plan")
                && explanation.planner_stages.contains(&"merge"),
            "explanation must surface the plan + merge planner stages, got {:?}",
            explanation.planner_stages
        );
        assert_eq!(
            explanation.engines_touched,
            vec!["lexical"],
            "explain route must touch exactly the lexical engine"
        );
        assert_eq!(explanation.strategy, "lexical_score_trace");
        assert_eq!(explanation.contributions_count, 1);
    }
}
