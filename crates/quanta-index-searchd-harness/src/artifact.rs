//! Benchmark artifact data model, JSON emission, and percentile math.
//!
//! This module is the bench-owned data model for emitting latency/shape
//! results. It carries no serde derives (banned repo-wide); all JSON is built
//! through `serde_json::json!` / manual `serde_json::Value` construction.

use std::io::Write as _;
use std::path::Path;

/// Route family a benchmarked scenario exercises.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RouteFamily {
    Lexical,
    History,
    RuntimeCatalog,
    Structural,
    Adversarial,
}

impl RouteFamily {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            RouteFamily::Lexical => "lexical",
            RouteFamily::History => "history",
            RouteFamily::RuntimeCatalog => "runtime_catalog",
            RouteFamily::Structural => "structural",
            RouteFamily::Adversarial => "adversarial",
        }
    }
}

/// Query syntax surface a scenario is phrased in.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BenchSyntax {
    Native,
    Sourcegraph,
}

impl BenchSyntax {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            BenchSyntax::Native => "native",
            BenchSyntax::Sourcegraph => "sourcegraph",
        }
    }
}

/// Cache-state mode under which the benchmark was taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BenchMode {
    Warm,
    Cold,
}

impl BenchMode {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            BenchMode::Warm => "warm",
            BenchMode::Cold => "cold",
        }
    }
}

/// Observed result shape produced by a scenario run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ResultShape {
    Candidates,
    Commits,
    DiffPaths,
    TypedError,
    Empty,
}

impl ResultShape {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            ResultShape::Candidates => "candidates",
            ResultShape::Commits => "commits",
            ResultShape::DiffPaths => "diff_paths",
            ResultShape::TypedError => "typed_error",
            ResultShape::Empty => "empty",
        }
    }
}

/// Nearest-rank latency percentiles over a sample set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatencySummary {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub samples: u32,
}

impl LatencySummary {
    /// Compute nearest-rank p50/p95/p99 over the given millisecond samples.
    ///
    /// Returns `None` for an empty slice. NaN values are ordered defensively
    /// via `f64::total_cmp` so the sort is total.
    #[must_use]
    pub fn from_samples_ms(samples: &[f64]) -> Option<LatencySummary> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted: Vec<f64> = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        Some(LatencySummary {
            p50_ms: nearest_rank(&sorted, 50),
            p95_ms: nearest_rank(&sorted, 95),
            p99_ms: nearest_rank(&sorted, 99),
            samples: saturating_u32(n),
        })
    }
}

/// Saturating narrowing of a `usize` sample count into the `u32` artifact
/// field. Benchmark sample counts never approach `u32::MAX`, so saturation is
/// a defensive ceiling rather than an expected path.
fn saturating_u32(n: usize) -> u32 {
    if let Ok(value) = u32::try_from(n) {
        return value;
    }
    u32::MAX
}

/// Nearest-rank percentile lookup on an ascending-sorted, non-empty slice.
///
/// index = ceil(p/100 * n) - 1, clamped to `[0, n-1]`.
fn nearest_rank(sorted: &[f64], p: u32) -> f64 {
    let n = sorted.len();
    // n >= 1 guaranteed by callers.
    let scaled = f64::from(p) / 100.0 * n_as_f64(n);
    let rank = f64_ceil_to_usize(scaled);
    let idx = rank.saturating_sub(1).min(n.saturating_sub(1));
    sorted.get(idx).copied().unwrap_or(f64::NAN)
}

/// Lossy `usize -> f64` widening for percentile scaling. Sample counts in
/// benchmarks never approach the f64 mantissa boundary, so precision loss is
/// not observable here.
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "benchmark sample counts are small; f64 widening is exact in range"
)]
fn n_as_f64(n: usize) -> f64 {
    n as f64
}

/// Truncate a non-negative ceil'd percentile value to a usize rank.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "scaled rank is non-negative and bounded by the sample count"
)]
fn f64_ceil_to_usize(value: f64) -> usize {
    value.ceil() as usize
}

/// One measured (or early-stopped) benchmark scenario row.
#[derive(Clone, Debug)]
pub struct BenchRow {
    pub scenario_id: String,
    pub route_family: RouteFamily,
    pub syntax: BenchSyntax,
    pub mode: BenchMode,
    pub result_shape: ResultShape,
    /// `None` when `early_stop_reason` is set (latency was not measured).
    pub latency: Option<LatencySummary>,
    pub result_count: Option<u64>,
    pub typed_error_code: Option<String>,
    pub engine_touched: Vec<String>,
    pub early_stop_reason: Option<String>,
}

impl BenchRow {
    /// Emit this row as a JSON object. `None` optionals serialize as `null`;
    /// when `latency` is `None` the four latency_* fields and `samples` are
    /// `null`.
    #[must_use]
    pub fn to_json(&self, git_rev: &str) -> serde_json::Value {
        let (p50, p95, p99, samples) = self.latency.as_ref().map_or(
            (
                serde_json::Value::Null,
                serde_json::Value::Null,
                serde_json::Value::Null,
                serde_json::Value::Null,
            ),
            |l| {
                (
                    serde_json::json!(l.p50_ms),
                    serde_json::json!(l.p95_ms),
                    serde_json::json!(l.p99_ms),
                    serde_json::json!(l.samples),
                )
            },
        );

        let result_count = self
            .result_count
            .map_or(serde_json::Value::Null, |c| serde_json::json!(c));
        let typed_error_code = self
            .typed_error_code
            .as_ref()
            .map_or(serde_json::Value::Null, |code| serde_json::json!(code));
        let early_stop_reason = self
            .early_stop_reason
            .as_ref()
            .map_or(serde_json::Value::Null, |reason| serde_json::json!(reason));
        let engine_touched: Vec<serde_json::Value> = self
            .engine_touched
            .iter()
            .map(|e| serde_json::json!(e))
            .collect();

        serde_json::json!({
            "scenario_id": self.scenario_id,
            "route_family": self.route_family.as_str(),
            "syntax": self.syntax.as_str(),
            "mode": self.mode.as_str(),
            "result_shape": self.result_shape.as_str(),
            "latency_p50_ms": p50,
            "latency_p95_ms": p95,
            "latency_p99_ms": p99,
            "samples": samples,
            "result_count": result_count,
            "typed_error_code": typed_error_code,
            "engine_touched": engine_touched,
            "early_stop_reason": early_stop_reason,
            "git_rev": git_rev,
        })
    }
}

/// Schema version stamped onto every emitted artifact.
pub const SCHEMA_VERSION: u32 = 1;

/// Top-level benchmark artifact: a versioned envelope over a set of rows.
#[derive(Clone, Debug)]
pub struct BenchArtifact {
    pub schema_version: u32,
    pub mode: BenchMode,
    pub git_rev: String,
    pub rows: Vec<BenchRow>,
}

impl BenchArtifact {
    /// Construct an empty artifact stamped with the current schema version.
    pub fn new(mode: BenchMode, git_rev: impl Into<String>) -> BenchArtifact {
        BenchArtifact {
            schema_version: SCHEMA_VERSION,
            mode,
            git_rev: git_rev.into(),
            rows: Vec::new(),
        }
    }

    /// Emit the full artifact as a JSON object.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let rows: Vec<serde_json::Value> =
            self.rows.iter().map(|r| r.to_json(&self.git_rev)).collect();
        serde_json::json!({
            "schema_version": self.schema_version,
            "mode": self.mode.as_str(),
            "git_rev": self.git_rev,
            "rows": rows,
        })
    }

    /// Serialize pretty-printed JSON (with a trailing newline) to `path`,
    /// creating parent directories if needed.
    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(&self.to_json())?;
        text.push('\n');
        let mut file = std::fs::File::create(path)?;
        file.write_all(text.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "percentile boundaries land on exact integer-valued samples"
    )]
    #[expect(
        clippy::expect_used,
        reason = "test asserts the Some invariant for a non-empty sample set"
    )]
    fn nearest_rank_on_one_to_hundred() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let summary = LatencySummary::from_samples_ms(&samples).expect("non-empty");
        // nearest-rank: idx = ceil(p/100 * 100) - 1 = p - 1 -> value p.
        assert_eq!(summary.p50_ms, 50.0);
        assert_eq!(summary.p95_ms, 95.0);
        assert_eq!(summary.p99_ms, 99.0);
        assert_eq!(summary.samples, 100);
    }

    #[test]
    fn empty_samples_yield_none() {
        assert!(LatencySummary::from_samples_ms(&[]).is_none());
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "single-sample percentiles collapse to the exact input value"
    )]
    #[expect(
        clippy::expect_used,
        reason = "test asserts the Some invariant for a non-empty sample set"
    )]
    fn single_element_collapses_all_percentiles() {
        let summary = LatencySummary::from_samples_ms(&[7.5]).expect("non-empty");
        assert_eq!(summary.p50_ms, 7.5);
        assert_eq!(summary.p95_ms, 7.5);
        assert_eq!(summary.p99_ms, 7.5);
        assert_eq!(summary.samples, 1);
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "test asserts the Some invariant for a non-empty sample set"
    )]
    fn nan_is_ordered_defensively() {
        // Should not panic; NaN sorts to the high end via total_cmp.
        let summary = LatencySummary::from_samples_ms(&[1.0, f64::NAN, 2.0]).expect("non-empty");
        assert_eq!(summary.samples, 3);
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "test asserts the JSON value is an object"
    )]
    fn measured_row_to_json_has_expected_keys() {
        let row = BenchRow {
            scenario_id: "lexical.keyword.native".to_string(),
            route_family: RouteFamily::Lexical,
            syntax: BenchSyntax::Native,
            mode: BenchMode::Warm,
            result_shape: ResultShape::Candidates,
            latency: LatencySummary::from_samples_ms(&[1.0, 2.0, 3.0, 4.0]),
            result_count: Some(12),
            typed_error_code: None,
            engine_touched: vec!["lexical".to_string()],
            early_stop_reason: None,
        };
        let v = row.to_json("deadbeef");
        let obj = v.as_object().expect("object");
        for key in [
            "scenario_id",
            "route_family",
            "syntax",
            "mode",
            "result_shape",
            "latency_p50_ms",
            "latency_p95_ms",
            "latency_p99_ms",
            "samples",
            "result_count",
            "typed_error_code",
            "engine_touched",
            "early_stop_reason",
            "git_rev",
        ] {
            assert!(obj.contains_key(key), "missing key {key}");
        }
        assert_eq!(obj["route_family"], serde_json::json!("lexical"));
        assert_eq!(obj["git_rev"], serde_json::json!("deadbeef"));
        assert!(obj["typed_error_code"].is_null());
        assert!(!obj["latency_p50_ms"].is_null());
        assert_eq!(obj["engine_touched"], serde_json::json!(["lexical"]));
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "test asserts the JSON value is an object"
    )]
    fn early_stop_row_emits_null_latency() {
        let row = BenchRow {
            scenario_id: "history.diff_added.native".to_string(),
            route_family: RouteFamily::History,
            syntax: BenchSyntax::Native,
            mode: BenchMode::Cold,
            result_shape: ResultShape::DiffPaths,
            latency: None,
            result_count: None,
            typed_error_code: Some("NOT_READY".to_string()),
            engine_touched: Vec::new(),
            early_stop_reason: Some("fixture_unavailable".to_string()),
        };
        let v = row.to_json("cafef00d");
        let obj = v.as_object().expect("object");
        assert!(obj["latency_p50_ms"].is_null());
        assert!(obj["latency_p95_ms"].is_null());
        assert!(obj["latency_p99_ms"].is_null());
        assert!(obj["samples"].is_null());
        assert!(obj["result_count"].is_null());
        assert_eq!(obj["typed_error_code"], serde_json::json!("NOT_READY"));
        assert_eq!(
            obj["early_stop_reason"],
            serde_json::json!("fixture_unavailable")
        );
        assert_eq!(obj["engine_touched"], serde_json::json!([]));
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "test asserts the JSON envelope is an object with a rows array"
    )]
    fn artifact_to_json_envelope() {
        let mut artifact = BenchArtifact::new(BenchMode::Warm, "rev123");
        assert_eq!(artifact.schema_version, SCHEMA_VERSION);
        artifact.rows.push(BenchRow {
            scenario_id: "lexical.keyword.native".to_string(),
            route_family: RouteFamily::Lexical,
            syntax: BenchSyntax::Native,
            mode: BenchMode::Warm,
            result_shape: ResultShape::Candidates,
            latency: LatencySummary::from_samples_ms(&[5.0]),
            result_count: Some(1),
            typed_error_code: None,
            engine_touched: vec!["lexical".to_string()],
            early_stop_reason: None,
        });
        let v = artifact.to_json();
        let obj = v.as_object().expect("object");
        assert_eq!(obj["schema_version"], serde_json::json!(1));
        assert_eq!(obj["mode"], serde_json::json!("warm"));
        assert_eq!(obj["git_rev"], serde_json::json!("rev123"));
        assert_eq!(obj["rows"].as_array().expect("array").len(), 1);
    }
}
