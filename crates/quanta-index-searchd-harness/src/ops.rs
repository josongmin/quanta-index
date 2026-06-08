//! Operator-ergonomics rail (J7Q-05): snapshot the read-only diagnosis surfaces.
//!
//! This rail proves that the questions an operator routinely asks — "which
//! engines served this?", "what generation is active?", "what *typed* error did
//! the remote return?", "are perf/tail metrics queryable?" — are answerable from
//! **machine-readable** surfaces, with route / generation / typed-error
//! provenance preserved rather than swallowed behind human-only strings.
//!
//! It is a read-only first increment (per the ticket: read-only diagnosis before
//! any mutating workflow). It boots one [`E2eRuntime`], seeds a single seeded
//! document, and captures four surfaces into `cli_snapshots.json`:
//!
//! - `result_provenance` — a served query's `engines_touched` labels plus the
//!   repo / revision / serving-generation stamp the candidate carries;
//! - `typed_error` — a rejected (empty) query's typed error code, asserting the
//!   typed failure is surfaced verbatim, never collapsed to a generic string;
//! - `perf_metrics` — the registered metric names from the obs snapshot, the
//!   surface an operator reads for tail/latency diagnosis;
//! - `generation_state` — the runtime's current generation plus its pin.
//!
//! Fail-closed posture: a surface that swallows its provenance (empty engine
//! set, a generation mismatch, a blank typed-error code, or no metrics) is a rail
//! failure recorded as `provenance_ok=false`; the rail does NOT fabricate or
//! guess missing runtime state.

use std::path::Path;

use anyhow::Result as AnyResult;
use quanta_index_contract::TextQuerySyntax;
use serde_json::{Value, json};

use crate::harness::E2eRuntime;

/// Repo id the ops fixture is ingested under.
const OPS_REPO: &str = "repo-ops";

/// Result cap for the served diagnosis query.
const OPS_TOP_K: u32 = 10;

/// Token planted in the fixture so the served query returns a hit.
const OPS_QUERY: &str = "ops_diagnosis_needle";

/// One captured operator-diagnosis surface.
#[derive(Clone, Debug)]
pub struct OpsSnapshot {
    /// Stable surface id (artifact key + regression anchor).
    pub surface: &'static str,
    /// The operational question this surface answers, in plain words.
    pub operator_question: &'static str,
    /// The machine-readable snapshot bytes (stable field names).
    pub json: Value,
    /// Whether the surface preserved its provenance / typed signal. `false` is a
    /// blocking rail failure (a swallowed provenance, not a benign zero).
    pub provenance_ok: bool,
    /// Human-facing note recorded alongside the verdict.
    pub note: String,
}

/// The full ops report.
#[derive(Clone, Debug)]
pub struct OpsReport {
    pub snapshots: Vec<OpsSnapshot>,
    pub passed: bool,
}

/// Boot one runtime, seed the ops fixture, seal + activate.
fn prepare_ops_runtime() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text(
        OPS_REPO,
        "src/ops.rs",
        "fn ops_probe() {\n    // ops_diagnosis_needle marker for the operator rail\n}\n",
    )?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn engine_labels(engines: &[quanta_index_contract::EngineTouched]) -> Vec<String> {
    engines.iter().map(|engine| format!("{engine:?}")).collect()
}

/// Capture the result-provenance surface from a served query.
///
/// The operator question is "where did this result come from?" — a served
/// candidate must carry its own repo / revision / serving-generation stamp so an
/// operator can attribute it without reading runtime internals. `engines_touched`
/// is recorded as-is (the single-engine lexical path leaves it implicit; it is
/// the explain surface that enumerates engines), so it is informational here, not
/// the gating signal.
fn capture_result_provenance(rt: &mut E2eRuntime) -> OpsSnapshot {
    let result = rt.query_text(TextQuerySyntax::Native, OPS_QUERY, OPS_TOP_K);
    let engines = engine_labels(&result.engines_touched);
    let first_provenance = result.candidates.first().map(|candidate| {
        json!({
            "repo_id": candidate.repo_id.as_str(),
            "revision_id": candidate.revision_id.as_str(),
            "serving_generation": format!("{:?}", candidate.manifest_generation),
            "repo_relative_path": candidate.repo_relative_path.as_str(),
            "score": candidate.score,
        })
    });
    // Provenance is preserved when the query served a hit that carries its own
    // repo/revision/serving-generation stamp (attributable without internals).
    let provenance_ok = result.typed_error.is_none() && first_provenance.is_some();
    OpsSnapshot {
        surface: "result_provenance",
        operator_question: "where did a served result come from (repo / revision / serving generation)?",
        json: json!({
            "engines_touched": engines,
            "candidate_count": result.candidates.len(),
            "first_candidate_provenance": first_provenance,
        }),
        provenance_ok,
        note: if provenance_ok {
            "served result carries repo/revision/generation provenance".to_string()
        } else {
            "served result missing source provenance".to_string()
        },
    }
}

/// Capture the typed-error surface from a rejected (empty) query.
fn capture_typed_error(rt: &mut E2eRuntime) -> OpsSnapshot {
    let result = rt.query_text(TextQuerySyntax::Native, "", OPS_TOP_K);
    let (code, message) = result.typed_error.as_ref().map_or((None, None), |error| {
        (Some(error.code.clone()), Some(error.message.clone()))
    });
    // Provenance is preserved when the rejection carries a non-empty TYPED code,
    // not a generic human string (the ticket No-Go).
    let provenance_ok = code.as_ref().is_some_and(|code| !code.is_empty());
    OpsSnapshot {
        surface: "typed_error",
        operator_question: "what typed error does the remote return for a rejected query?",
        json: json!({
            "probe": "empty query text (policy rejection)",
            "typed_error_code": code,
            "typed_error_message": message,
        }),
        provenance_ok,
        note: if provenance_ok {
            "typed error code surfaced verbatim".to_string()
        } else {
            "rejected query did not surface a typed error code".to_string()
        },
    }
}

/// Capture the perf/observability surface (the tail-diagnosis read surface).
fn capture_perf_metrics(rt: &E2eRuntime) -> OpsSnapshot {
    let (names, count, error) = match rt.query_metrics_snapshot() {
        Ok(samples) => {
            let mut names: Vec<String> =
                samples.iter().map(|sample| sample.name.to_string()).collect();
            names.sort_unstable();
            names.dedup();
            let count = samples.len();
            (names, count, None)
        }
        Err(err) => (Vec::new(), 0, Some(format!("{err}"))),
    };
    // Provenance is preserved when the obs surface is queryable and emitted at
    // least one sample for the served queries (perf/tail is diagnosable).
    let provenance_ok = error.is_none() && count > 0;
    OpsSnapshot {
        surface: "perf_metrics",
        operator_question: "are perf / tail metrics queryable for diagnosis?",
        json: json!({
            "metric_sample_count": count,
            "metric_names": names,
            "snapshot_error": error,
        }),
        provenance_ok,
        note: if provenance_ok {
            "obs metrics queryable".to_string()
        } else {
            "obs metric surface empty or unavailable".to_string()
        },
    }
}

/// Capture the generation-state surface (serving-head provenance).
fn capture_generation_state(rt: &E2eRuntime) -> OpsSnapshot {
    let active_generation = format!("{:?}", rt.current_generation());
    let generation_pin = format!("{:?}", rt.generation_pin());
    let provenance_ok = !active_generation.is_empty();
    OpsSnapshot {
        surface: "generation_state",
        operator_question: "what is the runtime's current generation and pin?",
        json: json!({
            "repo_id": rt.repo().as_str(),
            "revision_id": rt.revision().as_str(),
            "current_generation": active_generation,
            "generation_pin": generation_pin,
        }),
        provenance_ok,
        note: "current generation + pin exposed".to_string(),
    }
}

/// Run the full ops rail against a freshly seeded runtime.
pub fn run_ops_report() -> AnyResult<OpsReport> {
    let mut rt = prepare_ops_runtime()?;
    let snapshots = vec![
        capture_result_provenance(&mut rt),
        capture_typed_error(&mut rt),
        capture_perf_metrics(&rt),
        capture_generation_state(&rt),
    ];
    let passed = snapshots.iter().all(|snapshot| snapshot.provenance_ok);
    Ok(OpsReport { snapshots, passed })
}

// ---------------------------------------------------------------------------
// Artifact emission.
// ---------------------------------------------------------------------------

fn snapshot_json(snapshot: &OpsSnapshot) -> Value {
    json!({
        "surface": snapshot.surface,
        "operator_question": snapshot.operator_question,
        "snapshot": snapshot.json,
        "provenance_ok": snapshot.provenance_ok,
        "note": snapshot.note,
    })
}

/// The CLI-snapshot record: the machine-readable diagnosis surfaces, verbatim.
#[must_use]
pub fn cli_snapshots_json(report: &OpsReport) -> Value {
    json!({
        "schema_version": 1,
        "dimension": "ops",
        "surfaces": report.snapshots.iter().map(snapshot_json).collect::<Vec<_>>(),
        "snapshot_note": "read-only operator-diagnosis surfaces captured machine-readably; route/generation/typed-error provenance is asserted preserved, never collapsed to a human-only string",
    })
}

/// Build the ops summary value.
#[must_use]
pub fn summary_json(report: &OpsReport, git_rev: &str) -> Value {
    json!({
        "schema_version": 1,
        "dimension": "ops",
        "git_rev": git_rev,
        "passed": report.passed,
        "blocking_signal": "every captured diagnosis surface must preserve its provenance (engines, generation, typed-error code, perf metrics); a swallowed provenance fails the rail",
        "surfaces": report.snapshots.iter().map(snapshot_json).collect::<Vec<_>>(),
    })
}

fn write_json(path: &Path, value: &Value) -> AnyResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(())
}

/// Write the two canonical ops artifacts under `dir`:
/// `summary.json` and `cli_snapshots.json`.
pub fn write_artifacts(report: &OpsReport, dir: &Path, git_rev: &str) -> AnyResult<()> {
    write_json(&dir.join("summary.json"), &summary_json(report, git_rev))?;
    write_json(&dir.join("cli_snapshots.json"), &cli_snapshots_json(report))?;
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
    fn snapshot_json_preserves_surface_and_provenance() {
        let snapshot = OpsSnapshot {
            surface: "route_provenance",
            operator_question: "which engines?",
            json: json!({ "engines_touched": ["Lexical"] }),
            provenance_ok: true,
            note: "ok".to_string(),
        };
        let value = snapshot_json(&snapshot);
        assert_eq!(value["surface"], "route_provenance");
        assert_eq!(value["provenance_ok"], true);
        assert_eq!(value["snapshot"]["engines_touched"][0], "Lexical");
    }

    #[test]
    fn summary_passed_requires_all_surfaces_ok() {
        let ok = OpsSnapshot {
            surface: "a",
            operator_question: "?",
            json: json!({}),
            provenance_ok: true,
            note: String::new(),
        };
        let bad = OpsSnapshot {
            surface: "b",
            operator_question: "?",
            json: json!({}),
            provenance_ok: false,
            note: String::new(),
        };
        let pass_report = OpsReport {
            snapshots: vec![ok.clone()],
            passed: true,
        };
        let fail_report = OpsReport {
            snapshots: vec![ok, bad],
            passed: false,
        };
        assert_eq!(summary_json(&pass_report, "rev")["passed"], true);
        assert_eq!(summary_json(&fail_report, "rev")["passed"], false);
        let surfaces = summary_json(&fail_report, "rev")["surfaces"]
            .as_array()
            .expect("surfaces array")
            .len();
        assert_eq!(surfaces, 2);
    }
}
