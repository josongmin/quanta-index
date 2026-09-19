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
measured — their latency is null. Such rows are skipped entirely: never
compared, never failed.
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


@dataclass(frozen=True)
class Artifact:
    mode: str
    git_head: str
    config_digest: str
    rows: dict[str, ScenarioRow]


class ArtifactRefused(Exception):
    """The artifact is not one this gate can compare."""


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
    return None if value is None else float(value)


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
    rows: dict[str, ScenarioRow] = {}
    for row in payload.get("rows", []):
        scenario_id = row["scenario_id"]
        latency = row.get("latency")
        if latency is None:
            latency = {}
        rows[scenario_id] = ScenarioRow(
            scenario_id=scenario_id,
            route_family=row["route_family"],
            syntax=row["syntax"],
            result_shape=row["result_shape"],
            latency_p50_ms=_optional_float(latency, "p50_ms"),
            latency_p95_ms=_optional_float(latency, "p95_ms"),
            latency_p99_ms=_optional_float(latency, "p99_ms"),
            early_stop_reason=row.get("early_stop_reason"),
            samples=int(latency.get("samples", 0)),
        )
    return Artifact(mode=mode, git_head=git_head, config_digest=config_digest, rows=rows)


def is_measured(row: ScenarioRow) -> bool:
    return row.early_stop_reason is None and row.latency_p95_ms is not None


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
            f"({blocking_metric} rel > {rel_threshold * 100:.0f}% AND abs > {abs_threshold_ms:.2f}ms)."
        )
        print("To accept a deliberate change: re-run with --update-baseline.")
        return 1

    print("OK: no DSL latency regressions over thresholds.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
