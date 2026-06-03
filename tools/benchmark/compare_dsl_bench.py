#!/usr/bin/env python3
"""Compare a fresh DSL query-latency artifact against a committed baseline.

Layer-3 (query latency) regression gate for the DSL-benchmarking model defined
in ``docs/plans/jun-2-dsl-hardening/RFC-DSL-Benchmarking.md``. Reads two JSON
artifacts (see that doc / ``tools/benchmark/README.md`` for the schema): one
baseline (checked into ``tools/benchmark/baselines/``) and one current run
(produced by the criterion ``dsl_query_matrix`` bench for warm, or by
``run_dsl_cold_matrix.py`` for cold). Matches scenarios by ``scenario_id`` and
exits non-zero if any scenario's p95 latency has grown past the configured
relative + absolute thresholds.

Why both thresholds: noise on sub-millisecond warm scenarios easily produces
large relative swings; we only care about meaningful regressions, so a delta
must exceed *both* ``--rel-threshold`` and ``--abs-threshold-ms`` to count.

Rows carrying ``early_stop_reason`` (e.g. ``fixture_not_seeded``) were never
measured — their latency fields are null. Such rows are skipped entirely: never
compared, never failed.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path

# Mode-aware default thresholds applied to the p95 latency metric. Explicit
# --rel-threshold / --abs-threshold-ms flags override these.
DEFAULT_REL_THRESHOLD = 0.10
DEFAULT_ABS_THRESHOLD_MS = {"warm": 1.0, "cold": 5.0}
MIN_SAMPLES_FOR_P95 = {"cold": 20}


@dataclass(frozen=True)
class ScenarioRow:
    scenario_id: str
    route_family: str
    syntax: str
    mode: str
    result_shape: str
    latency_p95_ms: float | None
    early_stop_reason: str | None
    samples: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", type=Path, help="committed baseline artifact JSON")
    parser.add_argument(
        "current",
        type=Path,
        help="fresh artifact JSON (criterion dsl_query_matrix or run_dsl_cold_matrix.py)",
    )
    parser.add_argument(
        "--update-baseline",
        action="store_true",
        help="overwrite baseline with current and exit 0",
    )
    parser.add_argument(
        "--rel-threshold",
        type=float,
        default=None,
        help="relative p95 growth threshold (default: 0.10 = 10%%)",
    )
    parser.add_argument(
        "--abs-threshold-ms",
        type=float,
        default=None,
        help="absolute p95 growth threshold in ms (default: warm 1.0ms, cold 5.0ms)",
    )
    parser.add_argument(
        "--allow-missing",
        action="store_true",
        help="downgrade scenarios missing from current from FAIL to a warning",
    )
    return parser.parse_args()


def load_artifact(path: Path) -> tuple[str, dict[str, ScenarioRow]]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    mode = payload.get("mode", "")
    rows: dict[str, ScenarioRow] = {}
    for row in payload.get("rows", []):
        scenario_id = row["scenario_id"]
        latency = row.get("latency_p95_ms")
        rows[scenario_id] = ScenarioRow(
            scenario_id=scenario_id,
            route_family=row["route_family"],
            syntax=row["syntax"],
            mode=row["mode"],
            result_shape=row["result_shape"],
            latency_p95_ms=None if latency is None else float(latency),
            early_stop_reason=row.get("early_stop_reason"),
            samples=int(row.get("samples", 0)),
        )
    return mode, rows


def is_measured(row: ScenarioRow) -> bool:
    return row.early_stop_reason is None and row.latency_p95_ms is not None


def main() -> int:
    args = parse_args()

    if args.update_baseline:
        args.baseline.parent.mkdir(parents=True, exist_ok=True)
        args.baseline.write_text(
            args.current.read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        print(f"updated baseline {args.baseline}")
        return 0

    base_mode, baseline = load_artifact(args.baseline)
    cur_mode, current = load_artifact(args.current)

    if base_mode != cur_mode:
        print(
            f"ERROR: mode mismatch: baseline mode={base_mode!r} current mode={cur_mode!r}",
            file=sys.stderr,
        )
        return 2

    mode = cur_mode
    rel_threshold = args.rel_threshold if args.rel_threshold is not None else DEFAULT_REL_THRESHOLD
    abs_threshold_ms = (
        args.abs_threshold_ms
        if args.abs_threshold_ms is not None
        else DEFAULT_ABS_THRESHOLD_MS.get(mode, 1.0)
    )

    regressed: list[tuple[str, float, float, float, float]] = []
    missing_measured: list[str] = []
    insufficient_samples: list[tuple[str, str, int, int]] = []

    all_scenarios = sorted(set(baseline) | set(current))
    print(
        f"mode={mode}  rel-threshold={rel_threshold * 100:.0f}%  abs-threshold={abs_threshold_ms:.2f}ms"
    )
    print(f"{'scenario':40s} {'baseline':>10s} {'current':>10s} {'delta':>10s} {'rel':>8s}")
    print("-" * 82)
    for scenario_id in all_scenarios:
        base = baseline.get(scenario_id)
        cur = current.get(scenario_id)

        if base is None:
            # New scenario: present in current, absent from baseline. Never fails.
            note = "skipped" if cur is not None and not is_measured(cur) else "NEW"
            print(f"{scenario_id:40s} {'--':>10s} {'--':>10s} {'--':>10s} {note:>8s}")
            continue

        if cur is None:
            # Missing from current. Only meaningful if the baseline row was measured.
            if is_measured(base):
                missing_measured.append(scenario_id)
                marker = " <-- MISSING"
            else:
                marker = ""
            print(f"{scenario_id:40s} {'--':>10s} {'--':>10s} {'--':>10s} {'gone':>8s}{marker}")
            continue

        if not is_measured(base) or not is_measured(cur):
            reason = cur.early_stop_reason or base.early_stop_reason or "unmeasured"
            print(f"{scenario_id:40s} {'--':>10s} {'--':>10s} {'--':>10s} {'skip':>8s}  ({reason})")
            continue

        min_samples = MIN_SAMPLES_FOR_P95.get(mode)
        if min_samples is not None and (base.samples < min_samples or cur.samples < min_samples):
            insufficient_samples.append((scenario_id, mode, base.samples, cur.samples))
            print(
                f"{scenario_id:40s} {'--':>10s} {'--':>10s} {'--':>10s} "
                f"{'invalid':>8s}  (samples base={base.samples} current={cur.samples}; need >= {min_samples})"
            )
            continue

        base_ms = base.latency_p95_ms
        cur_ms = cur.latency_p95_ms
        assert base_ms is not None and cur_ms is not None
        delta = cur_ms - base_ms
        rel = (delta / base_ms) if base_ms > 0 else float("inf") if delta > 0 else 0.0
        rel_pct = f"{rel * 100:+.1f}%" if base_ms > 0 else "  inf"
        marker = ""
        if rel > rel_threshold and delta > abs_threshold_ms:
            regressed.append((scenario_id, base_ms, cur_ms, delta, rel))
            marker = " <-- REGRESSION"
        print(
            f"{scenario_id:40s} {base_ms:>8.2f}ms {cur_ms:>8.2f}ms "
            f"{delta:>+8.2f}ms {rel_pct:>8s}{marker}"
        )

    failures = 0
    if regressed:
        print()
        for scenario_id, base_ms, cur_ms, delta, rel in regressed:
            print(
                f"REGRESSION {scenario_id}: p95 {base_ms:.2f}ms -> {cur_ms:.2f}ms "
                f"({delta:+.2f}ms, {rel * 100:+.1f}%)"
            )
        failures += len(regressed)

    if missing_measured:
        print()
        for scenario_id in missing_measured:
            level = "WARN" if args.allow_missing else "MISSING"
            print(f"{level}: scenario {scenario_id!r} present in baseline but absent from current")
        if not args.allow_missing:
            failures += len(missing_measured)

    if insufficient_samples:
        print()
        for scenario_id, row_mode, base_samples, cur_samples in insufficient_samples:
            required = MIN_SAMPLES_FOR_P95.get(row_mode, 0)
            print(
                f"INVALID: scenario {scenario_id!r} has insufficient samples for p95 gating "
                f"(baseline={base_samples}, current={cur_samples}, required>={required})"
            )
        return 2

    print()
    if failures:
        regressed_n = len(regressed)
        missing_n = len(missing_measured) if not args.allow_missing else 0
        parts = []
        if regressed_n:
            parts.append(f"{regressed_n} regressed")
        if missing_n:
            parts.append(f"{missing_n} missing")
        print(
            f"FAIL: {', '.join(parts)} scenario(s) over thresholds "
            f"(p95 rel > {rel_threshold * 100:.0f}% AND abs > {abs_threshold_ms:.2f}ms)."
        )
        print("To accept a deliberate change: re-run with --update-baseline.")
        return 1

    print("OK: no DSL latency regressions over thresholds.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
