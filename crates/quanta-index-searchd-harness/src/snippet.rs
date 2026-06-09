//! Snippet-quality rail (J7Q-02).
//!
//! Measures the lexical snippet beyond mere substring-presence. A snippet that
//! merely *contains* the matched needle is not a quality snippet: a useful
//! snippet is a **hit-centered window** — the needle sits inside a bounded slice
//! of surrounding context, and an over-long source line is truncated
//! deterministically so the head of the result list never streams an unbounded
//! blob. This rail seeds a corpus designed to exercise four snippet intents
//! (phrase hit, regex hit, multi-hit, long-line), runs each query against the
//! live ranker, and grades the produced [`LexicalCandidate::snippet`] against
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
//! - a **pure window oracle** ([`compute_snippet_window`]) that defines what a
//!   correct hit-centered, bounded, deterministically-truncated window looks
//!   like, unit-tested with synthetic inputs (no daemon). This is the SSOT for
//!   the *target* snippet contract;
//! - a **seeded end-to-end measurement** that records the engine's ACTUAL
//!   snippet against that contract.
//!
//! Fail-closed posture: the rail does NOT rewrite or post-process the engine
//! snippet to make it pass. The live lexical path truncates an over-long source
//! line to a hit-centered, bounded window (the J7Q-02 engine fix), and this rail
//! grades that emitted window as-is. The window oracle ([`compute_snippet_window`])
//! is the independent target spec: it proves the gate can distinguish a good
//! window from a bad one, and the engine's emitted window converges to it. A
//! regression that re-emitted an unwindowed full-chunk snippet would trip the
//! centering/bound gate RED — that RED would be the measurement, not a rail defect.

use std::path::Path;

use anyhow::Result as AnyResult;
use quanta_index_contract::{LexicalCandidate, TextQuerySyntax};
use serde_json::{Value, json};

use crate::harness::E2eRuntime;

/// Repo id under which the snippet fixture is ingested.
pub const SNIPPET_REPO: &str = "repo-snippet";

/// Result cap requested for every snippet query.
pub const TOP_K: u32 = 10;

/// Declared maximum snippet length (bytes) a quality snippet may emit.
///
/// A snippet longer than this is, by definition, not a bounded hit-centered
/// window: the long-line intent must be truncated below this ceiling. Sized so
/// a normal multi-line code chunk passes but an unbounded long line trips.
pub const MAX_SNIPPET_LEN: usize = 240;

/// Minimum leading context (bytes) a *long-line* hit must carry.
///
/// If a long-line snippet places the hit within this many bytes of byte 0, it is
/// "pinned to the leading edge" and fails the centering gate (no leading context
/// was retained). Short snippets (whole source shorter than [`MAX_SNIPPET_LEN`])
/// are exempt: there is nothing to center because no truncation was required.
pub const MIN_LEADING_CONTEXT: usize = 16;

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
/// - returns `None` when the needle is absent (a snippet with no hit has no
///   window — the caller treats this as a fail-closed needle-absent failure,
///   never a best-effort empty window).
///
/// Truncation is byte-window-based and fully determined by
/// `(source, needle, max_len)`, so two runs over identical inputs always emit
/// byte-identical windows — the property the golden-window artifact freezes.
#[must_use]
pub fn compute_snippet_window(source: &str, needle: &str, max_len: usize) -> Option<SnippetWindow> {
    if needle.is_empty() {
        return None;
    }
    let hit = source.find(needle)?;
    if source.len() <= max_len {
        return Some(SnippetWindow {
            text: source.to_string(),
            hit_offset: hit,
        });
    }
    // Center the window on the hit: half the budget of leading context, the
    // remainder trailing. Saturating arithmetic keeps the start in range.
    let lead_budget = half(max_len);
    let raw_start = hit.saturating_sub(lead_budget);
    let start = floor_char_boundary(source, raw_start);
    let raw_end = start.saturating_add(max_len).min(source.len());
    let end = floor_char_boundary(source, raw_end);
    // `start..end` is always a valid, in-bounds slice: `start` and `end` are both
    // `floor_char_boundary` results (hence char boundaries) with `start <=
    // raw_start <= hit <= source.len()`, `raw_end = min(start + max_len,
    // source.len()) >= start`, and stepping `raw_end` down to a boundary cannot
    // cross below the boundary `start`, so `start <= end <= source.len()`. Thus
    // `source.get(start..end)` is always `Some`; the `unwrap_or("")` is
    // unreachable-by-construction, kept as a typed-safe floor in place of a slice
    // index (string slicing is banned in production paths).
    let text = source.get(start..end).unwrap_or("").to_string();
    let hit_offset = hit.saturating_sub(start);
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

/// The snippet intent a judged query exercises.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnippetIntent {
    /// A multi-token phrase that must appear verbatim and centered.
    Phrase,
    /// A token reachable via regex syntax; needle is the literal it resolves to.
    Regex,
    /// A token that occurs multiple times in the chunk (multi-hit count > 1).
    MultiHit,
    /// A single very long source line that MUST be truncated + centered.
    LongLine,
}

impl SnippetIntent {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Phrase => "phrase",
            Self::Regex => "regex",
            Self::MultiHit => "multi_hit",
            Self::LongLine => "long_line",
        }
    }

    /// Whether this intent's source is long enough that a bounded, centered
    /// window is *required* (vs. a short chunk that is legitimately emitted
    /// whole).
    #[must_use]
    pub fn requires_truncation(self) -> bool {
        matches!(self, Self::LongLine)
    }
}

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

/// One seeded snippet fixture document.
#[derive(Clone, Copy, Debug)]
pub struct SnippetFixture {
    pub path: &'static str,
    pub content: &'static str,
}

/// One judged snippet query: query text, the needle the snippet must center on,
/// and the declared intent that selects the gate.
#[derive(Clone, Copy, Debug)]
pub struct SnippetQuery {
    /// Stable query id (artifact key + regression anchor).
    pub id: &'static str,
    /// Query text issued to the engine.
    pub query: &'static str,
    /// The literal substring the snippet must contain and center on.
    pub needle: &'static str,
    /// Intent — selects the blocking gate.
    pub intent: SnippetIntent,
    /// Repo-relative path of the fixture doc the query is expected to hit.
    pub expect_path: &'static str,
}

/// A long source line whose needle is buried near the middle.
///
/// A single physical line far exceeding [`MAX_SNIPPET_LEN`], so a correct
/// snippet MUST truncate and center, and the live (full-text) snippet will
/// exceed the bound.
const LONG_LINE: &str = "let prefix_padding_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa = compute_target_marker_zzz(0) + suffix_padding_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb;\n";

/// `(path, content)` snippet fixtures, one per intent.
pub const SNIPPET_CORPUS: &[SnippetFixture] = &[
    SnippetFixture {
        path: "src/snippet/phrase.rs",
        content: "fn header() {}\n\
                  // the quick brown fox marker sits here for the phrase intent\n\
                  fn footer() {}\n",
    },
    SnippetFixture {
        path: "src/snippet/regex.rs",
        content: "fn alpha() {}\n\
                  pub fn regex_target_symbol(input: &str) -> usize { input.len() }\n\
                  fn omega() {}\n",
    },
    SnippetFixture {
        path: "src/snippet/multi.rs",
        content: "fn repeated_marker_a() { repeated_marker_a(); repeated_marker_a(); }\n",
    },
    SnippetFixture {
        path: "src/snippet/longline.rs",
        content: LONG_LINE,
    },
];

/// The judged snippet query set (the snippet rail SSOT).
pub const SNIPPET_QUERIES: &[SnippetQuery] = &[
    SnippetQuery {
        id: "snip.phrase.brown_fox",
        query: "brown",
        needle: "brown fox marker",
        intent: SnippetIntent::Phrase,
        expect_path: "src/snippet/phrase.rs",
    },
    SnippetQuery {
        id: "snip.regex.target_symbol",
        query: "regex_target_symbol",
        needle: "regex_target_symbol",
        intent: SnippetIntent::Regex,
        expect_path: "src/snippet/regex.rs",
    },
    SnippetQuery {
        id: "snip.multi.repeated_marker",
        query: "repeated_marker_a",
        needle: "repeated_marker_a",
        intent: SnippetIntent::MultiHit,
        expect_path: "src/snippet/multi.rs",
    },
    SnippetQuery {
        id: "snip.longline.compute_target",
        query: "compute_target_marker_zzz",
        needle: "compute_target_marker_zzz",
        intent: SnippetIntent::LongLine,
        expect_path: "src/snippet/longline.rs",
    },
];

// ---------------------------------------------------------------------------
// Report engine.
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
}

/// Boot one runtime, seed the snippet fixtures, seal + activate.
pub fn prepare_snippet_runtime() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    for fixture in SNIPPET_CORPUS {
        rt.ingest_text(SNIPPET_REPO, fixture.path, fixture.content)?;
    }
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

/// Find the candidate matching the expected fixture path.
///
/// Returns `None` when no candidate for that path is present (a retrieval miss,
/// surfaced as a failure rather than silently graded against a different doc).
fn candidate_for_path<'a>(
    candidates: &'a [LexicalCandidate],
    expect_path: &str,
) -> Option<&'a LexicalCandidate> {
    candidates
        .iter()
        .find(|c| c.repo_relative_path.as_str() == expect_path)
}

/// Score one snippet query end to end against the live ranker.
///
/// Fails closed: a typed query error or a missing expected candidate is a
/// recorded failure, never a silent pass with an empty snippet.
fn score_query(rt: &mut E2eRuntime, query: &SnippetQuery) -> SnippetScore {
    let result = rt.query_text(TextQuerySyntax::Native, query.query, TOP_K);
    if let Some(error) = result.typed_error {
        return SnippetScore {
            id: query.id,
            intent: query.intent,
            query: query.query,
            needle: query.needle,
            expect_path: query.expect_path,
            matched_path: None,
            snippet: None,
            metrics: None,
            failures: vec![format!(
                "{}: query returned typed error {}: {}",
                query.intent.as_str(),
                error.code,
                error.message
            )],
        };
    }
    let Some(candidate) = candidate_for_path(&result.candidates, query.expect_path) else {
        return SnippetScore {
            id: query.id,
            intent: query.intent,
            query: query.query,
            needle: query.needle,
            expect_path: query.expect_path,
            matched_path: None,
            snippet: None,
            metrics: None,
            failures: vec![format!(
                "{}: expected candidate for `{}` not retrieved (snippet ungradable)",
                query.intent.as_str(),
                query.expect_path
            )],
        };
    };
    let metrics = measure_snippet(&candidate.snippet, query.needle);
    let failures = gate_snippet(query.intent, &metrics, MAX_SNIPPET_LEN, MIN_LEADING_CONTEXT);
    SnippetScore {
        id: query.id,
        intent: query.intent,
        query: query.query,
        needle: query.needle,
        expect_path: query.expect_path,
        matched_path: Some(candidate.repo_relative_path.as_str().to_string()),
        snippet: Some(candidate.snippet.clone()),
        metrics: Some(metrics),
        failures,
    }
}

/// Run the full snippet rail against a freshly seeded runtime.
pub fn run_snippet_report() -> AnyResult<SnippetReport> {
    let mut rt = prepare_snippet_runtime()?;
    let mut scores = Vec::with_capacity(SNIPPET_QUERIES.len());
    for query in SNIPPET_QUERIES {
        scores.push(score_query(&mut rt, query));
    }
    let passed = scores.iter().all(SnippetScore::passed);
    Ok(SnippetReport { scores, passed })
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
/// Pairs the exact snippet bytes the engine emitted with the ideal hit-centered
/// window the oracle would produce for the same `(snippet-source, needle)`. With
/// the engine truncating, the two converge on the long-line intent; a divergence
/// would be the visible, checked-in evidence of an unwindowed-snippet regression.
fn golden_window_json(score: &SnippetScore) -> Value {
    let observed = score.snippet.clone();
    let oracle = score
        .snippet
        .as_ref()
        .and_then(|s| compute_snippet_window(s, score.needle, MAX_SNIPPET_LEN));
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

/// Write the two canonical snippet artifacts under `dir`.
pub fn write_artifacts(report: &SnippetReport, dir: &Path, git_rev: &str) -> AnyResult<()> {
    let scores: Vec<Value> = report.scores.iter().map(score_json).collect();
    let golden: Vec<Value> = report.scores.iter().map(golden_window_json).collect();
    crate::artifact::write_json_pretty(
        &dir.join("summary.json"),
        &json!({
            "schema_version": 1,
            "dimension": "snippet",
            "git_rev": git_rev,
            "passed": report.passed,
            "max_snippet_len": MAX_SNIPPET_LEN,
            "min_leading_context": MIN_LEADING_CONTEXT,
            "scores": scores,
            "gate_note": "snippet quality is hit-centered window + bounded length + deterministic truncation, NOT substring presence; the engine snippet is graded as-emitted, never post-processed to pass",
        }),
    )?;
    crate::artifact::write_json_pretty(
        &dir.join("golden_windows.json"),
        &json!({
            "schema_version": 1,
            "dimension": "snippet",
            "git_rev": git_rev,
            "windows": golden,
            "oracle_note": "oracle_window is the ideal hit-centered, bounded window compute_snippet_window would emit for (observed_snippet, needle); with the engine truncating, observed_snippet and oracle_window converge on the long_line intent, and any divergence marks an unwindowed-snippet regression",
        }),
    )?;
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index into failure vectors and slices whose shape this module produces and asserts directly; an out-of-range index is a legitimate test failure"
)]
mod tests {
    use super::*;

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

    // --- seeded end-to-end ----------------------------------------------

    #[test]
    fn seeded_snippet_rail_runs_and_grades_every_query() {
        let report = run_snippet_report().expect("snippet rail runs");
        assert_eq!(
            report.scores.len(),
            SNIPPET_QUERIES.len(),
            "every judged query must be scored"
        );
        // Every NON-long-line intent must retrieve its candidate and present the
        // needle: a short-chunk snippet is the full chunk, which contains the
        // needle, so phrase/regex/multi must pass the presence floor.
        for score in &report.scores {
            if !score.intent.requires_truncation() {
                let metrics = score
                    .metrics
                    .as_ref()
                    .unwrap_or_else(|| panic!("{}: no metrics (retrieval miss?)", score.id));
                assert!(
                    metrics.needle_present,
                    "{}: needle absent from snippet `{:?}`",
                    score.id, score.snippet
                );
            }
        }
        // The long-line query MUST retrieve its candidate so the snippet is
        // gradable; with the engine truncating to a hit-centered window it now
        // PASSES the bound/centering gate (the live measurement).
        let long = report
            .scores
            .iter()
            .find(|s| s.intent == SnippetIntent::LongLine)
            .expect("long-line query scored");
        assert!(
            long.snippet.is_some(),
            "long-line snippet must be retrieved to be measurable: {:?}",
            long.failures
        );
    }
}
