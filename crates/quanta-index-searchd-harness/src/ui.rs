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
//! Fail-closed posture: a missing anchor, an out-of-range offset, or an offset
//! that does not point at the needle is a rail failure recorded per probe — the
//! rail never fabricates an anchor to make the gate pass. It grades the emitted
//! contract as-is, proving the new typed field across the engine + contract
//! layers (not a single layer).

use std::path::Path;

use anyhow::Result as AnyResult;
use quanta_index_contract::{HighlightSpan, TextQuerySyntax};
use serde_json::{Value, json};

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

/// The full UI-contract report.
#[derive(Clone, Debug)]
pub struct UiReport {
    pub scores: Vec<UiScore>,
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

/// Run the full UI-contract rail against a freshly seeded runtime.
pub fn run_ui_report() -> AnyResult<UiReport> {
    let mut rt = prepare_ui_runtime()?;
    let mut scores = Vec::with_capacity(UI_PROBES.len());
    for probe in UI_PROBES {
        scores.push(score_probe(&mut rt, probe));
    }
    let passed = scores.iter().all(UiScore::passed);
    Ok(UiReport { scores, passed })
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

/// The consumer-facing contract snapshot: the typed UI fields per probe.
#[must_use]
pub fn contract_snapshots_json(report: &UiReport) -> Value {
    json!({
        "schema_version": 1,
        "dimension": "ui",
        "fields_under_test": ["snippet", "snippet_hit_offset", "highlights"],
        "snapshot_note": "snippet_hit_offset + highlights are the typed UI anchors (J7Q-07); a consumer slices snippet at offset/spans to land on hits, with no regex parsing of raw text",
        "snapshots": report.scores.iter().map(score_json).collect::<Vec<_>>(),
    })
}

/// Build the UI summary value.
#[must_use]
pub fn summary_json(report: &UiReport, git_rev: &str) -> Value {
    json!({
        "schema_version": 1,
        "dimension": "ui",
        "git_rev": git_rev,
        "passed": report.passed,
        "blocking_signal": "every served candidate must carry a typed snippet_hit_offset that points exactly at the matched needle; a missing/wrong anchor fails the rail",
        "scores": report.scores.iter().map(score_json).collect::<Vec<_>>(),
    })
}

/// Write the two canonical UI artifacts under `dir`:
/// `summary.json` and `contract_snapshots.json`.
pub fn write_artifacts(report: &UiReport, dir: &Path, git_rev: &str) -> AnyResult<()> {
    crate::artifact::write_json_pretty(&dir.join("summary.json"), &summary_json(report, git_rev))?;
    crate::artifact::write_json_pretty(
        &dir.join("contract_snapshots.json"),
        &contract_snapshots_json(report),
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
            passed: true,
        };
        let value = summary_json(&report, "deadbeef");
        assert_eq!(value["dimension"], "ui");
        assert_eq!(value["passed"], true);
        assert_eq!(value["scores"][0]["snippet_hit_offset"], 2);
        assert_eq!(value["scores"][0]["highlights"][0]["start"], 2);
        assert_eq!(value["scores"][0]["highlights"][0]["len"], 6);
        let snaps = contract_snapshots_json(&report);
        assert_eq!(snaps["fields_under_test"][1], "snippet_hit_offset");
        assert_eq!(snaps["fields_under_test"][2], "highlights");
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
                snippet.get(at..).is_some_and(|tail| tail.starts_with(score.needle)),
                "probe {}: offset {offset} must point at `{}` in snippet {snippet:?}",
                score.id,
                score.needle
            );
            // Every highlight span must cover the needle exactly, and the span
            // set must cover every occurrence (multi-hit coverage).
            assert!(!score.highlights.is_empty(), "{}: no highlight spans", score.id);
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
    }
}
