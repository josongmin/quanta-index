#!/usr/bin/env python3
"""Compare a fresh DSL query-latency artifact against a committed baseline.

Layer-3 (query latency) regression gate for the DSL-benchmarking model in
``docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md``.
Reads two
``BenchArtifactV1`` JSON artifacts (schema 2; see ``tools/benchmark/README.md``
and ``crates/quanta-index-searchd-harness/src/artifact.rs``): one baseline
(checked into ``tools/benchmark/baselines/``) and one current run (produced by
``dsl_warm_matrix`` for warm, or by ``run_dsl_cold_matrix.py`` for cold).
Matches scenarios by ``scenario_id`` and exits non-zero if the mode's
*blocking metric* has grown past the configured relative + absolute
thresholds.

Provenance gate (QI-BB-010, findings §9): both artifacts must be schema 2
and carry a full 40-character ``git_head``; the *current* artifact's head
must be the checkout's ``HEAD``, otherwise the comparison
measured some other source and is refused (exit 2). The baseline is a
reference captured at an earlier head, so it is held to the shape, not to
head equality. The two artifacts must also agree on ``config_digest``: a
comparison across different sample counts or scenario tables is not a
regression signal. A schema-1 baseline (short ``git_rev``, no digests) is
refused with instructions to re-capture it.

Blocking metrics:

- warm/cold ``p50``: central-tendency regression, ``>10%`` and ``>1ms`` / ``>5ms``
- warm/cold ``p95``: agent-loop tail regression, ``>20%`` and ``>5ms`` / ``>10ms``

``p99`` remains advisory.  A coding-agent turn commonly compounds several
retrieval calls, so allowing an unbounded p95 regression while only gating p50
would admit a user-visible regression.  The p95 policy is deliberately looser
than p50 to tolerate normal tail variance on the canonical host.

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
import math
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

if str(Path(__file__).resolve().parents[2]) not in sys.path:
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tools.benchmark.evidence import CONTROL_DOCUMENT_BYTES, _read_control_file  # noqa: E402

CURRENT_SCHEMA_VERSION = 2
FULL_HEAD_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")

# Mode-aware default thresholds applied to the blocking metric. Explicit
# --rel-threshold / --abs-threshold-ms flags override these.
DEFAULT_P50_REL_THRESHOLD = 0.10
DEFAULT_P50_ABS_THRESHOLD_MS = {"warm": 1.0, "cold": 5.0}
DEFAULT_P95_REL_THRESHOLD = 0.20
DEFAULT_P95_ABS_THRESHOLD_MS = {"warm": 5.0, "cold": 10.0}
BLOCKING_METRICS = ("p50", "p95")
MIN_SAMPLES_FOR_AUTHORITY = {"warm": 200, "cold": 20}


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
    result_count: int | None
    engine_touched: tuple[str, ...]


@dataclass(frozen=True)
class Artifact:
    mode: str
    git_head: str
    corpus_digest: str
    config_digest: str
    model_revision: str | None
    host_identity: tuple[str, str, int, int, str]
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
        help="legacy option; refused in favor of guarded benchctl baseline admission",
    )
    parser.add_argument(
        "--preflight-receipt",
        type=Path,
        help="legacy receipt option; standalone baseline admission is refused",
    )
    parser.add_argument(
        "--rel-threshold",
        type=float,
        default=None,
        help="relative growth threshold on p50 (default: 0.10 = 10%%)",
    )
    parser.add_argument(
        "--abs-threshold-ms",
        type=float,
        default=None,
        help="absolute growth threshold in ms on p50 (default: warm 1.0ms, cold 5.0ms)",
    )
    parser.add_argument(
        "--p95-rel-threshold",
        type=float,
        default=None,
        help="relative growth threshold on p95 (default: 0.20 = 20%%)",
    )
    parser.add_argument(
        "--p95-abs-threshold-ms",
        type=float,
        default=None,
        help="absolute growth threshold in ms on p95 (default: warm 5.0ms, cold 10.0ms)",
    )
    return parser.parse_args()


def resolve_head() -> str:
    """The checkout's full HEAD; refuses anything that is not one."""
    completed = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise ArtifactRefused(f"git rev-parse HEAD failed: {completed.stderr.strip()}")
    head = completed.stdout.strip()
    if not FULL_HEAD_RE.match(head):
        raise ArtifactRefused(f"head {head!r} is not 40 lowercase hex characters")
    return head


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


def load_artifact(path: Path, *, role: str, content: str | None = None) -> Artifact:
    """Decode one ``BenchArtifactV1``, refusing an old schema or a bad head."""
    try:
        if content is not None and (
            len(content) > CONTROL_DOCUMENT_BYTES
            or len(content.encode("utf-8")) > CONTROL_DOCUMENT_BYTES
        ):
            raise ValueError("native control document exceeds explicit byte limit")
        payload = json.loads(_read_control_file(path) if content is None else content)
    except FileNotFoundError:
        raise
    except (OSError, ValueError) as exc:
        # File custody wraps OS errors; preserve the public missing-artifact
        # classification using the actual cause, not a second pathname check.
        if isinstance(exc.__cause__, FileNotFoundError):
            raise exc.__cause__ from None
        raise ArtifactRefused(f"{role} {path} cannot be decoded: {exc}") from exc
    require(isinstance(payload, dict), f"{role} {path} is not a JSON object")
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
    corpus_digest = provenance.get("corpus_digest")
    for key, digest in (("corpus_digest", corpus_digest), ("config_digest", config_digest)):
        if not isinstance(digest, str) or not DIGEST_RE.match(digest):
            raise ArtifactRefused(f"{role} {path} {key} {digest!r} is not a sha256 digest")
    model_revision = provenance.get("model_revision")
    if model_revision is not None and (not isinstance(model_revision, str) or not model_revision):
        raise ArtifactRefused(f"{role} {path} model_revision is not a non-empty string or null")
    host = payload.get("host")
    if not isinstance(host, dict):
        raise ArtifactRefused(f"{role} {path} has no host")
    host_values = (
        host.get("os"),
        host.get("arch"),
        host.get("cpu_count"),
        host.get("mem_bytes"),
        host.get("hostname_hash"),
    )
    if (
        not isinstance(host_values[0], str)
        or not host_values[0]
        or not isinstance(host_values[1], str)
        or not host_values[1]
        or not isinstance(host_values[2], int)
        or isinstance(host_values[2], bool)
        or host_values[2] < 1
        or not isinstance(host_values[3], int)
        or isinstance(host_values[3], bool)
        or host_values[3] < 1
        or not isinstance(host_values[4], str)
        or not DIGEST_RE.match(host_values[4])
    ):
        raise ArtifactRefused(f"{role} {path} has an invalid host identity")
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
            raise ArtifactRefused(
                f"{role} {path} rows[{index}].early_stop_reason is not a string or null"
            )
        error_count = row.get("error_count")
        timeout_count = row.get("timeout_count")
        if not isinstance(error_count, int) or isinstance(error_count, bool) or error_count < 0:
            raise ArtifactRefused(
                f"{role} {path} rows[{index}].error_count is not a non-negative integer"
            )
        if (
            not isinstance(timeout_count, int)
            or isinstance(timeout_count, bool)
            or timeout_count < 0
        ):
            raise ArtifactRefused(
                f"{role} {path} rows[{index}].timeout_count is not a non-negative integer"
            )
        typed_error_code = row.get("typed_error_code")
        if typed_error_code is not None and (
            not isinstance(typed_error_code, str) or not typed_error_code
        ):
            raise ArtifactRefused(
                f"{role} {path} rows[{index}].typed_error_code is not a non-empty string or null"
            )
        result_count = row.get("result_count")
        if result_count is not None and (type(result_count) is not int or result_count < 0):
            raise ArtifactRefused(f"{role} {path} rows[{index}].result_count is invalid")
        engines = row.get("engine_touched")
        if not isinstance(engines, list) or not all(
            isinstance(engine, str) and engine for engine in engines
        ):
            raise ArtifactRefused(f"{role} {path} rows[{index}].engine_touched is invalid")
        if early_stop_reason is None:
            for key in ("p50_ms", "p95_ms", "p99_ms", "samples"):
                if key not in latency:
                    raise ArtifactRefused(
                        f"{role} {path} rows[{index}] is measured but latency.{key} is missing"
                    )
            samples = _required_samples(latency, role=role, path=path, index=index)
            p50 = _optional_float(latency, "p50_ms")
            p95 = _optional_float(latency, "p95_ms")
            p99 = _optional_float(latency, "p99_ms")
            require(
                p50 is not None and p95 is not None and p99 is not None and p50 <= p95 <= p99,
                f"{role} {path} rows[{index}] has missing or unordered latency percentiles",
            )
        else:
            if row.get("latency") is not None:
                raise ArtifactRefused(
                    f"{role} {path} rows[{index}] has early_stop_reason but non-null latency"
                )
            samples = 0
            p50 = p95 = p99 = None
        rows[scenario_id] = ScenarioRow(
            scenario_id=scenario_id,
            route_family=row["route_family"],
            syntax=row["syntax"],
            result_shape=row["result_shape"],
            latency_p50_ms=p50,
            latency_p95_ms=p95,
            latency_p99_ms=p99,
            early_stop_reason=early_stop_reason,
            samples=samples,
            error_count=error_count,
            timeout_count=timeout_count,
            typed_error_code=typed_error_code,
            result_count=result_count,
            engine_touched=tuple(engines),
        )
    return Artifact(
        mode=mode,
        git_head=git_head,
        corpus_digest=corpus_digest,
        config_digest=config_digest,
        model_revision=model_revision,
        host_identity=(
            host_values[0],
            host_values[1],
            host_values[2],
            host_values[3],
            host_values[4],
        ),
        rows=rows,
    )


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
    if baseline.corpus_digest != current.corpus_digest:
        raise ArtifactRefused(
            "corpus_digest mismatch: baseline "
            f"{baseline.corpus_digest} vs current {current.corpus_digest}; "
            "a comparison across different corpus bytes is not a regression signal"
        )
    if baseline.model_revision != current.model_revision:
        raise ArtifactRefused(
            "model_revision mismatch: baseline "
            f"{baseline.model_revision!r} vs current {current.model_revision!r}; "
            "a comparison across different embedding models is not a regression signal"
        )
    if baseline.host_identity != current.host_identity:
        raise ArtifactRefused(
            "host identity mismatch: baseline "
            f"{baseline.host_identity!r} vs current {current.host_identity!r}; "
            "capture and compare on the same pinned benchmark host"
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
            "result_count",
            "engine_touched",
        ):
            if getattr(base, field) != getattr(cur, field):
                raise ArtifactRefused(
                    f"scenario {scenario_id!r} changed {field}: "
                    f"baseline={getattr(base, field)!r} current={getattr(cur, field)!r}; "
                    "recapture a reviewed baseline after confirming semantic equivalence"
                )


def require_complete_baseline_candidate(artifact: Artifact) -> None:
    """A baseline must ratchet every declared scenario, not preserve gaps."""
    if artifact.host_identity[0] != "linux":
        raise ArtifactRefused("baseline candidate was not captured on the canonical Linux host")
    for scenario_id, row in artifact.rows.items():
        if not is_measured(row):
            raise ArtifactRefused(
                f"baseline candidate scenario {scenario_id!r} is unmeasured; "
                "repair the fixture before accepting a baseline"
            )


def require_clean_host_load(receipt: dict[str, object]) -> None:
    """Recompute the timing load guard instead of trusting a clean status label."""
    host = receipt.get("host")
    contention = receipt.get("host_contention")
    if not isinstance(host, dict) or not isinstance(contention, dict):
        raise ArtifactRefused("preflight host load evidence is missing")
    cpu_count = host.get("cpu_count")
    loads = host.get("load_average")
    if (
        type(cpu_count) is not int
        or cpu_count <= 0
        or not isinstance(loads, list)
        or len(loads) != 3
    ):
        raise ArtifactRefused("preflight host load evidence is invalid")
    if any(
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(value)
        or value < 0
        for value in loads
    ):
        raise ArtifactRefused("preflight host load evidence is invalid")
    limit = cpu_count * 0.5
    if (
        loads[0] >= limit
        or contention.get("one_minute_load") != float(loads[0])
        or contention.get("one_minute_load_limit") != limit
        or contention.get("over_limit") is not False
    ):
        raise ArtifactRefused("preflight host load is over limit or inconsistent")


def require_clean_preflight(receipt_path: Path | None, artifact: Artifact) -> None:
    """Bind a baseline ratchet to a clean preflight on the measured host."""
    if receipt_path is None:
        raise ArtifactRefused("baseline admission requires a preflight receipt")
    try:
        receipt = json.loads(_read_control_file(receipt_path))
    except (OSError, ValueError) as exc:
        raise ArtifactRefused(f"cannot load preflight receipt {receipt_path}: {exc}") from exc
    if not isinstance(receipt, dict):
        raise ArtifactRefused("preflight receipt is not an object")
    if receipt.get("schema_version") != 1 or receipt.get("kind") != "quanta-index-timing-preflight":
        raise ArtifactRefused("preflight receipt is not quanta-index-timing-preflight schema 1")
    if receipt.get("run_id") != "benchctl:dsl-authority":
        raise ArtifactRefused("preflight receipt is not bound to dsl-authority")
    if receipt.get("status") != "clean":
        raise ArtifactRefused(f"preflight receipt status {receipt.get('status')!r} is not clean")
    if receipt.get("foreign_rust_processes") != []:
        raise ArtifactRefused("clean preflight receipt still names foreign Rust processes")
    require_clean_host_load(receipt)
    host = receipt.get("host")
    if not isinstance(host, dict):
        raise ArtifactRefused("preflight receipt host is not an object")
    expected = (
        artifact.host_identity[0],
        artifact.host_identity[1],
        artifact.host_identity[2],
        artifact.host_identity[4],
    )
    actual = (host.get("os"), host.get("arch"), host.get("cpu_count"), host.get("hostname_hash"))
    if actual != expected:
        raise ArtifactRefused(
            f"preflight host {actual!r} does not match candidate host {expected!r}"
        )


def atomically_write_baseline(destination: Path, content: str) -> None:
    """Durably replace one baseline without exposing a truncated file."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=destination.parent, prefix=f".{destination.name}.", delete=False
    ) as handle:
        temporary = Path(handle.name)
        try:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
    try:
        os.replace(temporary, destination)
        fsync_directory(destination.parent)
    except OSError:
        temporary.unlink(missing_ok=True)
        raise


def fsync_directory(directory: Path) -> None:
    descriptor = os.open(directory, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def require_no_pending_admission(baseline: Path) -> None:
    """A crash during the two-file update must invalidate both baselines."""
    marker = baseline.parent / ".dsl-admission-pending"
    try:
        marker.lstat()
    except FileNotFoundError:
        return
    except OSError as exc:
        raise ArtifactRefused(f"cannot inspect DSL admission marker {marker}: {exc}") from exc
    raise ArtifactRefused(f"DSL baseline admission is incomplete: {marker}")


def main() -> int:
    args = parse_args()

    if args.update_baseline:
        print(
            "ERROR: standalone baseline promotion cannot bind a receipt to the capture; "
            "use benchctl run dsl-authority --admit-baseline",
            file=sys.stderr,
        )
        return 2

    for flag, value in (
        ("--rel-threshold", args.rel_threshold),
        ("--abs-threshold-ms", args.abs_threshold_ms),
        ("--p95-rel-threshold", args.p95_rel_threshold),
        ("--p95-abs-threshold-ms", args.p95_abs_threshold_ms),
    ):
        if value is not None and (not math.isfinite(value) or value < 0):
            print(f"ERROR: {flag} must be finite and non-negative", file=sys.stderr)
            return 2

    try:
        head = resolve_head()
        current = load_artifact(args.current, role="current")
        if current.git_head != head:
            raise ArtifactRefused(
                f"current artifact git_head {current.git_head} is not HEAD {head}: "
                "stale artifact, re-run the rail at HEAD"
            )
        require_no_pending_admission(args.baseline)
        baseline = load_artifact(args.baseline, role="baseline")
        gate_provenance(baseline, current, head)
    except FileNotFoundError as exc:
        print(
            f"ERROR: {exc.filename}: no such artifact; capture one with the rail at HEAD "
            "(use benchctl run dsl-authority --admit-baseline)",
            file=sys.stderr,
        )
        return 2
    except ArtifactRefused as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2

    mode = current.mode
    p50_rel_threshold = (
        args.rel_threshold if args.rel_threshold is not None else DEFAULT_P50_REL_THRESHOLD
    )
    p50_abs_threshold_ms = (
        args.abs_threshold_ms
        if args.abs_threshold_ms is not None
        else DEFAULT_P50_ABS_THRESHOLD_MS.get(mode, 1.0)
    )
    p95_rel_threshold = (
        args.p95_rel_threshold if args.p95_rel_threshold is not None else DEFAULT_P95_REL_THRESHOLD
    )
    p95_abs_threshold_ms = (
        args.p95_abs_threshold_ms
        if args.p95_abs_threshold_ms is not None
        else DEFAULT_P95_ABS_THRESHOLD_MS.get(mode, 5.0)
    )
    thresholds = {
        "p50": (p50_rel_threshold, p50_abs_threshold_ms),
        "p95": (p95_rel_threshold, p95_abs_threshold_ms),
    }

    regressed: list[tuple[str, str, float, float, float, float]] = []
    missing_measured: list[str] = []
    unmeasured_current: list[tuple[str, str]] = []
    new_scenarios: list[str] = []
    insufficient_samples: list[tuple[str, str, int, int]] = []
    advisories: list[tuple[str, str, float, float, float, float]] = []

    all_scenarios = sorted(set(baseline.rows) | set(current.rows))
    print(
        f"mode={mode}  p50-threshold={p50_rel_threshold * 100:.0f}%/{p50_abs_threshold_ms:.2f}ms  "
        f"p95-threshold={p95_rel_threshold * 100:.0f}%/{p95_abs_threshold_ms:.2f}ms  "
        f"baseline-head={baseline.git_head[:12]}  "
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
            print(
                f"{scenario_id:40s} {'--':>10s} {'--':>10s} {'--':>10s} {'INVALID':>8s}  ({reason})"
            )
            continue

        min_samples = MIN_SAMPLES_FOR_AUTHORITY.get(mode)
        if min_samples is not None and (base.samples < min_samples or cur.samples < min_samples):
            insufficient_samples.append((scenario_id, mode, base.samples, cur.samples))
            print(
                f"{scenario_id:40s} {'--':>10s} {'--':>10s} {'--':>10s} "
                f"{'invalid':>8s}  (samples base={base.samples} current={cur.samples}; need >= {min_samples})"
            )
            continue

        base_ms = metric_value(base, "p50")
        cur_ms = metric_value(cur, "p50")
        assert base_ms is not None and cur_ms is not None
        delta = cur_ms - base_ms
        rel = (delta / base_ms) if base_ms > 0 else float("inf") if delta > 0 else 0.0
        rel_pct = f"{rel * 100:+.1f}%" if base_ms > 0 else "  inf"
        marker = ""
        p50_rel_threshold, p50_abs_threshold_ms = thresholds["p50"]
        if rel > p50_rel_threshold and delta > p50_abs_threshold_ms:
            regressed.append((scenario_id, "p50", base_ms, cur_ms, delta, rel))
            marker = " <-- P50_REGRESSION"
        print(
            f"{scenario_id:40s} {base_ms:>8.2f}ms {cur_ms:>8.2f}ms "
            f"{delta:>+8.2f}ms {rel_pct:>8s}{marker}"
        )

        for metric in BLOCKING_METRICS:
            if metric == "p50":
                continue
            base_metric = metric_value(base, metric)
            cur_metric = metric_value(cur, metric)
            assert base_metric is not None and cur_metric is not None
            metric_delta = cur_metric - base_metric
            metric_rel = (
                (metric_delta / base_metric)
                if base_metric > 0
                else float("inf")
                if metric_delta > 0
                else 0.0
            )
            metric_rel_threshold, metric_abs_threshold_ms = thresholds[metric]
            if metric_rel > metric_rel_threshold and metric_delta > metric_abs_threshold_ms:
                regressed.append(
                    (scenario_id, metric, base_metric, cur_metric, metric_delta, metric_rel)
                )

        base_advisory = metric_value(base, "p99")
        cur_advisory = metric_value(cur, "p99")
        assert base_advisory is not None and cur_advisory is not None
        advisory_delta = cur_advisory - base_advisory
        advisory_rel = (
            (advisory_delta / base_advisory)
            if base_advisory > 0
            else float("inf")
            if advisory_delta > 0
            else 0.0
        )
        if advisory_rel > p95_rel_threshold and advisory_delta > p95_abs_threshold_ms:
            advisories.append(
                (
                    scenario_id,
                    "p99",
                    base_advisory,
                    cur_advisory,
                    advisory_delta,
                    advisory_rel,
                )
            )

    failures = 0
    if regressed:
        print()
        for scenario_id, metric, base_ms, cur_ms, delta, rel in regressed:
            print(
                f"REGRESSION {scenario_id}: {metric} {base_ms:.2f}ms -> {cur_ms:.2f}ms "
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
            print(f"MISSING: scenario {scenario_id!r} present in baseline but absent from current")
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
            required = MIN_SAMPLES_FOR_AUTHORITY.get(row_mode, 0)
            print(
                f"INVALID: scenario {scenario_id!r} has insufficient samples for authority comparison "
                f"(baseline={base_samples}, current={cur_samples}, required>={required})"
            )
        return 2

    print()
    if failures:
        regressed_n = len(regressed)
        missing_n = len(missing_measured)
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
        print(f"FAIL: {', '.join(parts)} scenario(s) over p50/p95 thresholds.")
        print("To accept a deliberate change: run benchctl run dsl-authority --admit-baseline.")
        return 1

    print("OK: no DSL latency regressions over thresholds.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
