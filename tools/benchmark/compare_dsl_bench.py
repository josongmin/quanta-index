#!/usr/bin/env python3
"""Compare a fresh DSL query-latency artifact against a committed baseline.

Layer-3 (query latency) regression gate for the DSL-benchmarking model defined
in ``docs/plans/jun-2-dsl-hardening/RFC-DSL-Benchmarking.md``. Reads two
``BenchArtifactV1`` JSON artifacts (schema 2; see ``tools/benchmark/README.md``
and ``crates/quanta-index-searchd-harness/src/artifact.rs``): one baseline
(checked into ``tools/benchmark/baselines/``) and one current run (produced by
``dsl_warm_matrix`` for warm, or by ``run_dsl_cold_matrix.py`` for cold).
Matches scenarios by ``scenario_id`` and exits non-zero if the mode's
*blocking metric* has grown past the configured relative + absolute
thresholds.

Provenance gate (QI-BB-010, findings §9): both artifacts must be schema 2
and carry a full 40-character ``git_head``; the *current* artifact's head
must be the checkout's ``HEAD`` (or ``--head``), otherwise the comparison
measured some other source and is refused (exit 2). The baseline is a
reference captured at an earlier head, so it is held to the shape, not to
head equality. The two artifacts must also agree on ``config_digest``: a
comparison across different sample counts or scenario tables is not a
regression signal. A schema-1 baseline (short ``git_rev``, no digests) is
refused with instructions to re-capture it.

Blocking metrics:

- warm: ``p50`` (steady-state central tendency)
- cold: ``p50`` (first-query central tendency)

``p95``/``p99`` remain in the artifact as observability signals, but they are
advisory-only because same-commit workstation reruns still show ambient tail
drift long after the central tendency has stabilized.

Why both thresholds: noise on sub-millisecond scenarios easily produces large
relative swings; we only care about meaningful regressions, so a delta must
exceed *both* ``--rel-threshold`` and ``--abs-threshold-ms`` to count.

Rows carrying ``early_stop_reason`` (e.g. ``fixture_not_seeded``) were never
measured — their latency is null. An authority comparison fails closed on such
a current row (and refuses a baseline containing one): a missing measurement
is not evidence that latency did not regress.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

CURRENT_SCHEMA_VERSION = 2
FULL_HEAD_RE = re.compile(r"^[0-9a-f]{40}$")

# Mode-aware default thresholds applied to the blocking metric. Explicit
# --rel-threshold / --abs-threshold-ms flags override these.
DEFAULT_REL_THRESHOLD = 0.10
DEFAULT_ABS_THRESHOLD_MS = {"warm": 1.0, "cold": 5.0}
DEFAULT_BLOCKING_METRIC = {"warm": "p50", "cold": "p50"}
MIN_SAMPLES_FOR_P95 = {"cold": 20}


@dataclass(frozen=True)
class ScenarioRow:
    scenario_id: str
    route_family: str
    syntax: str
    result_shape: str
    latency_p50_ms: float | None
    latency_p95_ms: float | None
    latency_p99_ms: float | None
    early_stop_reason: str | None
    samples: int
    error_count: int
    timeout_count: int
    typed_error_code: str | None


@dataclass(frozen=True)
class Artifact:
    mode: str
    git_head: str
    config_digest: str
    rows: dict[str, ScenarioRow]


class ArtifactRefused(Exception):
    """The artifact is not one this gate can compare."""


def require(condition: bool, message: str) -> None:
    """Refuse malformed benchmark input before deriving a verdict from it."""
    if not condition:
        raise ArtifactRefused(message)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", type=Path, help="committed baseline artifact JSON")
    parser.add_argument(
        "current",
        type=Path,
        help="fresh artifact JSON (dsl_warm_matrix or run_dsl_cold_matrix.py)",
    )
    parser.add_argument(
        "--update-baseline",
        action="store_true",
        help="overwrite baseline with current (after the provenance gate) and exit 0",
    )
    parser.add_argument(
        "--rel-threshold",
        type=float,
        default=None,
        help="relative growth threshold on the blocking metric (default: 0.10 = 10%%)",
    )
    parser.add_argument(
        "--abs-threshold-ms",
        type=float,
        default=None,
        help="absolute growth threshold in ms (default: warm 1.0ms, cold 5.0ms)",
    )
    parser.add_argument(
        "--allow-missing",
        action="store_true",
        help="downgrade scenarios missing from current from FAIL to a warning",
    )
    parser.add_argument(
        "--head",
        default=None,
        help="the head the current artifact must carry (default: `git rev-parse HEAD`)",
    )
    return parser.parse_args()


def resolve_head(explicit: str | None) -> str:
    """The checkout's full HEAD; refuses anything that is not one."""
    if explicit is None:
        completed = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            check=False,
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0:
            raise ArtifactRefused(f"git rev-parse HEAD failed: {completed.stderr.strip()}")
        explicit = completed.stdout.strip()
    if not FULL_HEAD_RE.match(explicit):
        raise ArtifactRefused(f"head {explicit!r} is not 40 lowercase hex characters")
    return explicit


def _optional_float(row: dict, key: str) -> float | None:
    value = row.get(key)
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ArtifactRefused(f"latency.{key} is not a finite number or null")
    value = float(value)
    if value < 0 or value == float("inf") or value != value:
        raise ArtifactRefused(f"latency.{key} is not a finite non-negative number")
    return value


def _required_samples(latency: dict, *, role: str, path: Path, index: int) -> int:
    samples = latency.get("samples")
    if not isinstance(samples, int) or isinstance(samples, bool) or samples < 1:
        raise ArtifactRefused(
            f"{role} {path} rows[{index}] is measured but latency.samples is not a positive integer"
        )
    return samples


def load_artifact(path: Path, *, role: str) -> Artifact:
    """Decode one ``BenchArtifactV1``, refusing an old schema or a bad head."""
    payload = json.loads(path.read_text(encoding="utf-8"))
    schema = payload.get("schema_version")
    if schema != CURRENT_SCHEMA_VERSION:
        raise ArtifactRefused(
            f"{role} {path} is schema_version {schema!r}, not {CURRENT_SCHEMA_VERSION}; "
            "re-capture it with the current rail (a schema-1 artifact carries a short "
            "git_rev and no corpus/config digest, host or resources)"
        )
    provenance = payload.get("provenance")
    if not isinstance(provenance, dict):
        raise ArtifactRefused(f"{role} {path} has no provenance")
    git_head = provenance.get("git_head")
    if not isinstance(git_head, str) or not FULL_HEAD_RE.match(git_head):
        raise ArtifactRefused(
            f"{role} {path} git_head {git_head!r} is not 40 lowercase hex characters"
        )
    config_digest = provenance.get("config_digest")
    if not isinstance(config_digest, str) or not config_digest:
        raise ArtifactRefused(f"{role} {path} has no config_digest")
    mode = payload.get("mode")
    if mode not in ("warm", "cold"):
        raise ArtifactRefused(f"{role} {path} mode {mode!r} is not warm or cold")
    expected_dimension = f"dsl-{mode}"
    if payload.get("dimension") != expected_dimension:
        raise ArtifactRefused(
            f"{role} {path} dimension {payload.get('dimension')!r} is not {expected_dimension!r}"
        )
    raw_rows = payload.get("rows")
    if not isinstance(raw_rows, list) or not raw_rows:
        raise ArtifactRefused(f"{role} {path} has no benchmark rows")
    rows: dict[str, ScenarioRow] = {}
    for index, row in enumerate(raw_rows):
        if not isinstance(row, dict):
            raise ArtifactRefused(f"{role} {path} rows[{index}] is not an object")
        scenario_id = row.get("scenario_id")
        if not isinstance(scenario_id, str) or not scenario_id:
            raise ArtifactRefused(f"{role} {path} rows[{index}] has no scenario_id")
        if scenario_id in rows:
            raise ArtifactRefused(f"{role} {path} repeats scenario_id {scenario_id!r}")
        latency = row.get("latency")
        if latency is not None and not isinstance(latency, dict):
            raise ArtifactRefused(f"{role} {path} rows[{index}].latency is not an object or null")
        if latency is None:
            latency = {}
        for key in ("route_family", "syntax", "result_shape"):
            require(
                isinstance(row.get(key), str) and bool(row[key]),
                f"{role} {path} rows[{index}] has invalid {key}",
            )
        early_stop_reason = row.get("early_stop_reason")
        if early_stop_reason is not None and not isinstance(early_stop_reason, str):
            raise ArtifactRefused(f"{role} {path} rows[{index}].early_stop_reason is not a string or null")
        error_count = row.get("error_count")
        timeout_count = row.get("timeout_count")
        if not isinstance(error_count, int) or isinstance(error_count, bool) or error_count < 0:
            raise ArtifactRefused(f"{role} {path} rows[{index}].error_count is not a non-negative integer")
        if not isinstance(timeout_count, int) or isinstance(timeout_count, bool) or timeout_count < 0:
            raise ArtifactRefused(f"{role} {path} rows[{index}].timeout_count is not a non-negative integer")
        typed_error_code = row.get("typed_error_code")
        if typed_error_code is not None and (
            not isinstance(typed_error_code, str) or not typed_error_code
        ):
            raise ArtifactRefused(
                f"{role} {path} rows[{index}].typed_error_code is not a non-empty string or null"
            )
        if early_stop_reason is None:
            for key in ("p50_ms", "p95_ms", "p99_ms", "samples"):
                if key not in latency:
                    raise ArtifactRefused(
                        f"{role} {path} rows[{index}] is measured but latency.{key} is missing"
                    )
            samples = _required_samples(latency, role=role, path=path, index=index)
        else:
            if row.get("latency") is not None:
                raise ArtifactRefused(
                    f"{role} {path} rows[{index}] has early_stop_reason but non-null latency"
                )
            samples = 0
        rows[scenario_id] = ScenarioRow(
            scenario_id=scenario_id,
            route_family=row["route_family"],
            syntax=row["syntax"],
            result_shape=row["result_shape"],
            latency_p50_ms=_optional_float(latency, "p50_ms"),
            latency_p95_ms=_optional_float(latency, "p95_ms"),
            latency_p99_ms=_optional_float(latency, "p99_ms"),
            early_stop_reason=early_stop_reason,
            samples=samples,
            error_count=error_count,
            timeout_count=timeout_count,
            typed_error_code=typed_error_code,
        )
    return Artifact(mode=mode, git_head=git_head, config_digest=config_digest, rows=rows)


def is_measured(row: ScenarioRow) -> bool:
    return (
        row.early_stop_reason is None
        and row.latency_p50_ms is not None
        and row.latency_p95_ms is not None
        and row.latency_p99_ms is not None
        and row.samples > 0
    )


def metric_value(row: ScenarioRow, metric: str) -> float | None:
    if metric == "p50":
        return row.latency_p50_ms
    if metric == "p95":
        return row.latency_p95_ms
    if metric == "p99":
        return row.latency_p99_ms
    raise ValueError(f"unknown metric: {metric}")


def gate_provenance(baseline: Artifact, current: Artifact, head: str) -> None:
    """Refuse a comparison the provenance does not support."""
    if current.git_head != head:
        raise ArtifactRefused(
            f"current artifact git_head {current.git_head} is not HEAD {head}: "
            "stale artifact, re-run the rail at HEAD"
        )
    if baseline.mode != current.mode:
        raise ArtifactRefused(
            f"mode mismatch: baseline mode={baseline.mode!r} current mode={current.mode!r}"
        )
    if baseline.config_digest != current.config_digest:
        raise ArtifactRefused(
            "config_digest mismatch: baseline "
            f"{baseline.config_digest} vs current {current.config_digest}; "
            "a comparison across different run configurations is not a regression signal"
        )
    for scenario_id, row in baseline.rows.items():
        if not is_measured(row):
            raise ArtifactRefused(
                f"baseline scenario {scenario_id!r} is unmeasured; recapture a complete baseline"
            )
    for scenario_id in sorted(set(baseline.rows) & set(current.rows)):
        base = baseline.rows[scenario_id]
        cur = current.rows[scenario_id]
        for field in (
            "route_family",
            "syntax",
            "result_shape",
            "typed_error_code",
            "error_count",
            "timeout_count",
        ):
            if getattr(base, field) != getattr(cur, field):
                raise ArtifactRefused(
                    f"scenario {scenario_id!r} changed {field}: "
                    f"baseline={getattr(base, field)!r} current={getattr(cur, field)!r}; "
                    "recapture a reviewed baseline after confirming semantic equivalence"
                )


def require_complete_baseline_candidate(artifact: Artifact) -> None:
    """A baseline must ratchet every declared scenario, not preserve gaps."""
    for scenario_id, row in artifact.rows.items():
        if not is_measured(row):
            raise ArtifactRefused(
                f"baseline candidate scenario {scenario_id!r} is unmeasured; "
                "repair the fixture before accepting a baseline"
            )


def main() -> int:
    args = parse_args()

    try:
        head = resolve_head(args.head)
        current = load_artifact(args.current, role="current")
        if current.git_head != head:
            raise ArtifactRefused(
                f"current artifact git_head {current.git_head} is not HEAD {head}: "
                "stale artifact, re-run the rail at HEAD"
            )
        if args.update_baseline:
            require_complete_baseline_candidate(current)
            args.baseline.parent.mkdir(parents=True, exist_ok=True)
            args.baseline.write_text(
                args.current.read_text(encoding="utf-8"),
                encoding="utf-8",
            )
            print(f"updated baseline {args.baseline} at head {head}")
            return 0
        baseline = load_artifact(args.baseline, role="baseline")
        gate_provenance(baseline, current, head)
    except FileNotFoundError as exc:
        print(
            f"ERROR: {exc.filename}: no such artifact; capture one with the rail at HEAD "
            "(`--update-baseline` records a baseline)",
            file=sys.stderr,
        )
        return 2
    except ArtifactRefused as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2

    mode = current.mode
    rel_threshold = args.rel_threshold if args.rel_threshold is not None else DEFAULT_REL_THRESHOLD
    abs_threshold_ms = (
        args.abs_threshold_ms
        if args.abs_threshold_ms is not None
        else DEFAULT_ABS_THRESHOLD_MS.get(mode, 1.0)
    )
    blocking_metric = DEFAULT_BLOCKING_METRIC.get(mode, "p95")

    regressed: list[tuple[str, float, float, float, float]] = []
    missing_measured: list[str] = []
    unmeasured_current: list[tuple[str, str]] = []
    new_scenarios: list[str] = []
    insufficient_samples: list[tuple[str, str, int, int]] = []
    advisories: list[tuple[str, str, float, float, float, float]] = []

    all_scenarios = sorted(set(baseline.rows) | set(current.rows))
    print(
        f"mode={mode}  blocking-metric={blocking_metric}  rel-threshold={rel_threshold * 100:.0f}%  "
        f"abs-threshold={abs_threshold_ms:.2f}ms  baseline-head={baseline.git_head[:12]}  "
        f"current-head={current.git_head[:12]}"
    )
    print(f"{'scenario':40s} {'baseline':>10s} {'current':>10s} {'delta':>10s} {'rel':>8s}")
    print("-" * 82)
    for scenario_id in all_scenarios:
        base = baseline.rows.get(scenario_id)
        cur = current.rows.get(scenario_id)

        if base is None:
            # A new scenario has no ratchet reference. It must be explicitly
            # admitted by a reviewed baseline update, never silently pass.
            new_scenarios.append(scenario_id)
            print(f"{scenario_id:40s} {'--':>10s} {'--':>10s} {'--':>10s} {'NEW':>8s}")
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

        if not is_measured(cur):
            reason = cur.early_stop_reason or "incomplete latency row"
            unmeasured_current.append((scenario_id, reason))
            print(f"{scenario_id:40s} {'--':>10s} {'--':>10s} {'--':>10s} {'INVALID':>8s}  ({reason})")
            continue

        min_samples = MIN_SAMPLES_FOR_P95.get(mode)
        if min_samples is not None and (base.samples < min_samples or cur.samples < min_samples):
            insufficient_samples.append((scenario_id, mode, base.samples, cur.samples))
            print(
                f"{scenario_id:40s} {'--':>10s} {'--':>10s} {'--':>10s} "
                f"{'invalid':>8s}  (samples base={base.samples} current={cur.samples}; need >= {min_samples})"
            )
            continue

        base_ms = metric_value(base, blocking_metric)
        cur_ms = metric_value(cur, blocking_metric)
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

        for advisory_metric in ("p95", "p99"):
            if advisory_metric == blocking_metric:
                continue
            base_advisory = metric_value(base, advisory_metric)
            cur_advisory = metric_value(cur, advisory_metric)
            if base_advisory is None or cur_advisory is None:
                continue
            advisory_delta = cur_advisory - base_advisory
            advisory_rel = (
                (advisory_delta / base_advisory)
                if base_advisory > 0
                else float("inf")
                if advisory_delta > 0
                else 0.0
            )
            if advisory_rel > rel_threshold and advisory_delta > abs_threshold_ms:
                advisories.append(
                    (
                        scenario_id,
                        advisory_metric,
                        base_advisory,
                        cur_advisory,
                        advisory_delta,
                        advisory_rel,
                    )
                )

    failures = 0
    if regressed:
        print()
        for scenario_id, base_ms, cur_ms, delta, rel in regressed:
            print(
                f"REGRESSION {scenario_id}: {blocking_metric} {base_ms:.2f}ms -> {cur_ms:.2f}ms "
                f"({delta:+.2f}ms, {rel * 100:+.1f}%)"
            )
        failures += len(regressed)

    if advisories:
        print()
        for scenario_id, metric, base_ms, cur_ms, delta, rel in advisories:
            print(
                f"ADVISORY {scenario_id}: {metric} {base_ms:.2f}ms -> {cur_ms:.2f}ms "
                f"({delta:+.2f}ms, {rel * 100:+.1f}%)"
            )

    if missing_measured:
        print()
        for scenario_id in missing_measured:
            level = "WARN" if args.allow_missing else "MISSING"
            print(f"{level}: scenario {scenario_id!r} present in baseline but absent from current")
        if not args.allow_missing:
            failures += len(missing_measured)

    if new_scenarios:
        print()
        for scenario_id in new_scenarios:
            print(f"NEW: scenario {scenario_id!r} has no reviewed baseline")
        failures += len(new_scenarios)

    if unmeasured_current:
        print()
        for scenario_id, reason in unmeasured_current:
            print(f"INVALID: scenario {scenario_id!r} was not measured ({reason})")
        failures += len(unmeasured_current)

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
        new_n = len(new_scenarios)
        unmeasured_n = len(unmeasured_current)
        parts = []
        if regressed_n:
            parts.append(f"{regressed_n} regressed")
        if missing_n:
            parts.append(f"{missing_n} missing")
        if new_n:
            parts.append(f"{new_n} new without baseline")
        if unmeasured_n:
            parts.append(f"{unmeasured_n} unmeasured")
        print(
            f"FAIL: {', '.join(parts)} scenario(s) over thresholds "
            f"({blocking_metric} rel > {rel_threshold * 100:.0f}% AND abs > {abs_threshold_ms:.2f}ms)."
        )
        print("To accept a deliberate change: re-run with --update-baseline.")
        return 1

    print("OK: no DSL latency regressions over thresholds.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
