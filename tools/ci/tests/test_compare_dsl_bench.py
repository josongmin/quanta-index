"""Tests for tools/benchmark/compare_dsl_bench.py and run_dsl_cold_matrix.py.

Covers the Layer-3 DSL query-latency regression gate:

1. percentile helper (run_dsl_cold_matrix.py, nearest-rank)
2. no-regression OK case (exit 0)
3. clear regression: rel > 10% AND abs > threshold (exit 1)
4. AND-gate: rel exceeded but abs not exceeded -> OK
5. rows with early_stop_reason are skipped (never compared / failed)
6. --update-baseline overwrites and exits 0
7. mode mismatch -> exit 2
8. NEW scenario never fails
9. MISSING scenario fails without --allow-missing, warns with it
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
COMPARE_PATH = REPO_ROOT / "tools" / "benchmark" / "compare_dsl_bench.py"
COLD_MATRIX_PATH = REPO_ROOT / "tools" / "benchmark" / "run_dsl_cold_matrix.py"


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


COMPARE = _load_module("compare_dsl_bench", COMPARE_PATH)
COLD_MATRIX = _load_module("run_dsl_cold_matrix", COLD_MATRIX_PATH)


def _row(
    scenario_id: str,
    p95: float | None,
    *,
    route_family: str = "lexical",
    syntax: str = "native",
    mode: str = "warm",
    result_shape: str = "candidates",
    early_stop_reason: str | None = None,
) -> dict:
    p50 = None if p95 is None else p95 * 0.8
    p99 = None if p95 is None else p95 * 1.1
    return {
        "scenario_id": scenario_id,
        "route_family": route_family,
        "syntax": syntax,
        "mode": mode,
        "result_shape": result_shape,
        "latency_p50_ms": p50,
        "latency_p95_ms": p95,
        "latency_p99_ms": p99,
        "samples": 200,
        "result_count": 3,
        "typed_error_code": None,
        "engine_touched": [route_family],
        "early_stop_reason": early_stop_reason,
    }


def _write_artifact(path: Path, mode: str, rows: list[dict]) -> None:
    payload = {
        "schema_version": 1,
        "mode": mode,
        "git_rev": "deadbee",
        "rows": rows,
    }
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def _run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(COMPARE_PATH), *args],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )


# ---------------------------------------------------------------------------
# percentile helper (run_dsl_cold_matrix.py)
# ---------------------------------------------------------------------------


def test_percentile_nearest_rank_basic() -> None:
    samples = [10.0, 20.0, 30.0, 40.0, 50.0]
    # ceil(p/100 * n) - 1
    assert COLD_MATRIX.percentile(samples, 50) == 30.0  # ceil(2.5)-1 = 2
    assert COLD_MATRIX.percentile(samples, 95) == 50.0  # ceil(4.75)-1 = 4
    assert COLD_MATRIX.percentile(samples, 99) == 50.0
    assert COLD_MATRIX.percentile(samples, 100) == 50.0


def test_percentile_single_sample() -> None:
    assert COLD_MATRIX.percentile([7.5], 50) == 7.5
    assert COLD_MATRIX.percentile([7.5], 95) == 7.5
    assert COLD_MATRIX.percentile([7.5], 99) == 7.5


def test_percentile_unsorted_input_is_sorted() -> None:
    samples = [50.0, 10.0, 40.0, 20.0, 30.0]
    assert COLD_MATRIX.percentile(samples, 50) == 30.0
    assert COLD_MATRIX.percentile(samples, 95) == 50.0


def test_percentile_low_pct_clamps_to_first() -> None:
    samples = [1.0, 2.0, 3.0, 4.0]
    # ceil(0.01) - 1 = 0
    assert COLD_MATRIX.percentile(samples, 1) == 1.0


def test_percentile_empty_raises() -> None:
    try:
        COLD_MATRIX.percentile([], 50)
    except ValueError:
        return
    raise AssertionError("expected ValueError on empty samples")


# ---------------------------------------------------------------------------
# Case: no regression -> OK exit 0
# ---------------------------------------------------------------------------


def test_no_regression_exits_zero(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    # +5% rel, +0.05ms abs -> under both legs
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.05)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "OK: no DSL latency regressions" in result.stdout
    assert "REGRESSION" not in result.stdout


# ---------------------------------------------------------------------------
# Case: clear regression -> exit 1
# ---------------------------------------------------------------------------


def test_clear_regression_exits_one(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 10.00)])
    # +50% rel (>10%) and +5.0ms abs (>1.0ms) -> regression
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 15.00)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "REGRESSION" in result.stdout
    assert "lexical.keyword.native" in result.stdout
    assert "FAIL" in result.stdout
    assert "--update-baseline" in result.stdout


# ---------------------------------------------------------------------------
# Case: AND-gate -> rel exceeded but abs NOT -> OK
# ---------------------------------------------------------------------------


def test_rel_exceeded_but_not_abs_is_ok(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    # warm abs threshold is 1.0ms. base 0.40ms -> +50% rel but only +0.20ms abs.
    _write_artifact(baseline, "warm", [_row("lexical.sub.native", 0.40)])
    _write_artifact(current, "warm", [_row("lexical.sub.native", 0.60)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "REGRESSION" not in result.stdout
    assert "OK" in result.stdout


def test_abs_exceeded_but_not_rel_is_ok(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    # base 100ms -> +2ms abs (>1.0) but only +2% rel (<10%) -> OK
    _write_artifact(baseline, "warm", [_row("lexical.big.native", 100.0)])
    _write_artifact(current, "warm", [_row("lexical.big.native", 102.0)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout
    assert "REGRESSION" not in result.stdout


# ---------------------------------------------------------------------------
# Case: early_stop_reason rows are skipped
# ---------------------------------------------------------------------------


def test_early_stop_rows_are_skipped(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        baseline,
        "warm",
        [_row("structural.def.native", None, early_stop_reason="fixture_not_seeded")],
    )
    # Even a huge "regression" must be ignored because rows are unmeasured.
    _write_artifact(
        current,
        "warm",
        [_row("structural.def.native", None, early_stop_reason="fixture_not_seeded")],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "REGRESSION" not in result.stdout
    assert "skip" in result.stdout


def test_current_early_stop_does_not_regress(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("history.commit.native", 1.00)])
    _write_artifact(
        current,
        "warm",
        [_row("history.commit.native", None, early_stop_reason="fixture_not_seeded")],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "REGRESSION" not in result.stdout


# ---------------------------------------------------------------------------
# Case: --update-baseline
# ---------------------------------------------------------------------------


def test_update_baseline_overwrites_and_exits_zero(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 10.00)])

    fail_result = _run(str(baseline), str(current))
    assert fail_result.returncode == 1, fail_result.stdout

    update_result = _run(str(baseline), str(current), "--update-baseline")
    assert update_result.returncode == 0, update_result.stdout + update_result.stderr
    assert "updated baseline" in update_result.stdout
    assert baseline.read_text(encoding="utf-8") == current.read_text(encoding="utf-8")

    rerun = _run(str(baseline), str(current))
    assert rerun.returncode == 0, rerun.stdout
    assert "OK" in rerun.stdout


# ---------------------------------------------------------------------------
# Case: mode mismatch -> exit 2
# ---------------------------------------------------------------------------


def test_mode_mismatch_exits_two(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    _write_artifact(current, "cold", [_row("lexical.keyword.native", 1.00, mode="cold")])
    result = _run(str(baseline), str(current))
    assert result.returncode == 2, result.stdout + result.stderr
    assert "mode mismatch" in result.stderr


# ---------------------------------------------------------------------------
# Case: NEW scenario never fails
# ---------------------------------------------------------------------------


def test_new_scenario_never_fails(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(baseline, "warm", [_row("lexical.keyword.native", 1.00)])
    _write_artifact(
        current,
        "warm",
        [
            _row("lexical.keyword.native", 1.00),
            _row("lexical.brand.new", 999.0),
        ],
    )
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "lexical.brand.new" in result.stdout
    assert "NEW" in result.stdout
    assert "REGRESSION" not in result.stdout


# ---------------------------------------------------------------------------
# Case: MISSING scenario
# ---------------------------------------------------------------------------


def test_missing_scenario_fails_without_allow(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        baseline,
        "warm",
        [
            _row("lexical.keyword.native", 1.00),
            _row("lexical.gone.native", 2.00),
        ],
    )
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 1, result.stdout + result.stderr
    assert "MISSING" in result.stdout
    assert "lexical.gone.native" in result.stdout


def test_missing_scenario_warns_with_allow_missing(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    _write_artifact(
        baseline,
        "warm",
        [
            _row("lexical.keyword.native", 1.00),
            _row("lexical.gone.native", 2.00),
        ],
    )
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    result = _run(str(baseline), str(current), "--allow-missing")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "WARN" in result.stdout
    assert "OK" in result.stdout


def test_missing_unmeasured_baseline_scenario_does_not_fail(tmp_path: Path) -> None:
    baseline = tmp_path / "baseline.json"
    current = tmp_path / "current.json"
    # baseline scenario was never measured -> its absence is not a failure.
    _write_artifact(
        baseline,
        "warm",
        [
            _row("lexical.keyword.native", 1.00),
            _row("structural.def.native", None, early_stop_reason="fixture_not_seeded"),
        ],
    )
    _write_artifact(current, "warm", [_row("lexical.keyword.native", 1.00)])
    result = _run(str(baseline), str(current))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "OK" in result.stdout


if __name__ == "__main__":
    raise SystemExit(subprocess.call([sys.executable, "-m", "pytest", __file__, "-q"]))
