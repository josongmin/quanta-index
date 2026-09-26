#!/usr/bin/env python3
"""Refuse stale, unattributed or old-schema benchmark artifacts (QI-BB-010, findings §9).

Every benchmark/relevance artifact the harness emits is one ``BenchArtifactV1``
envelope (``crates/quanta-index-searchd-harness/src/artifact.rs``): schema
version 2, a ``provenance`` naming the exact 40-character ``git_head``, the
``corpus_digest`` and ``config_digest``, a ``model_revision`` (or ``null``),
a ``host``, the process ``resources``, the ``phases``, and ``rows`` carrying
p50/p95/p99, QPS and error/timeout counts. This gate walks the artifact
families below and refuses:

- an artifact whose ``schema_version`` is not the current one (a schema-1
  artifact carried a short ``git_rev`` and no digests, host or resources);
- an artifact whose ``git_head`` is not 40 lowercase hex characters;
- a fresh artifact (under ``artifacts/``) whose ``git_head`` is not the
  checkout's ``HEAD``: it measured some other source;
- an artifact missing any envelope, provenance, host, resource, phase or row
  field the contract names.

Committed baselines (``tools/benchmark/baselines/``) are references captured
at an earlier head, so they are held to the shape and the 40-character head
but not to head equality; ``compare_dsl_bench.py`` refuses a comparison whose
*current* side is not at ``HEAD``.

Absence is not staleness: a family with no artifact on disk is reported and
passes this gate (the perf evidence gate that requires the artifact to exist
is ``--require``, for the Linux perf runner). Exit codes: 0 clean, 1 refused,
2 usage.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
BENCHMARK_DIR = REPO_ROOT / "tools" / "benchmark"
if str(BENCHMARK_DIR) not in sys.path:
    sys.path.insert(0, str(BENCHMARK_DIR))

from manifest import (  # noqa: E402
    ManifestError,
    baseline_families,
    fresh_families,
    load_manifest,
    profile_families,
)
from native_contracts import validate_concurrency  # noqa: E402

#: The schema every benchmark artifact must carry, mirroring
#: ``BENCH_ARTIFACT_SCHEMA_VERSION`` in ``artifact.rs``.
CURRENT_SCHEMA_VERSION = 2

FULL_HEAD_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
POTION_CODE_MODEL_REVISION = (
    "model2vec:minishlab/potion-code-16M-v2@"
    "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:"
    "model2vec-rs-0.3.0:fancy-regex:full-length-v1:d256"
)

try:
    MANIFEST = load_manifest()
except ManifestError as exc:
    raise RuntimeError(f"invalid benchmark manifest: {exc}") from exc
FRESH_FAMILIES = fresh_families(MANIFEST)
BASELINE_FAMILIES = baseline_families(MANIFEST)
FAMILY_PROFILES = profile_families(MANIFEST)

ENVELOPE_KEYS = (
    "schema_version",
    "dimension",
    "mode",
    "concurrency",
    "provenance",
    "host",
    "resources",
    "phases",
    "disk_amplification",
    "rows",
    "detail",
)
PROVENANCE_KEYS = ("git_head", "corpus_digest", "config_digest", "model_revision")
HOST_KEYS = ("os", "arch", "cpu_count", "mem_bytes", "hostname_hash")
RESOURCE_KEYS = ("peak_rss_bytes",)
PHASE_KEYS = ("build_ms", "update_ms", "gc_ms")
DISK_AMPLIFICATION_KEYS = ("bytes_written", "changed_bytes", "ratio")
ROW_KEYS = (
    "scenario_id",
    "route_family",
    "syntax",
    "result_shape",
    "latency",
    "qps",
    "error_count",
    "timeout_count",
    "result_count",
    "typed_error_code",
    "engine_touched",
    "early_stop_reason",
)
LATENCY_KEYS = ("p50_ms", "p95_ms", "p99_ms", "samples")
MODES = ("warm", "cold")
UNTIMED_DIMENSIONS = ("relevance", "relevance-openai-ab", "ambiguity", "snippet", "ops", "ui")
CONCURRENCY_COUNTS = (1, 8, 32)


@dataclass(frozen=True)
class Refusal:
    path: Path
    reason: str

    def __str__(self) -> str:
        return f"{self.path}: {self.reason}"


def resolve_head(repo_root: Path) -> str:
    """The checkout's full HEAD, or a usage error when git cannot answer."""
    completed = subprocess.run(
        ["git", "-C", str(repo_root), "rev-parse", "HEAD"],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"git rev-parse HEAD failed: {completed.stderr.strip()}")
    head = completed.stdout.strip()
    if not FULL_HEAD_RE.match(head):
        raise RuntimeError(f"git rev-parse HEAD printed {head!r}, not a full head")
    return head


def require_clean_worktree(repo_root: Path) -> None:
    """Refuse current-source evidence after source has diverged from its HEAD."""
    completed = subprocess.run(
        ["git", "-C", str(repo_root), "status", "--porcelain", "--untracked-files=normal"],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"git status failed: {completed.stderr.strip()}")
    if completed.stdout:
        raise RuntimeError(
            "worktree is dirty: current-source benchmark evidence requires a clean checkout"
        )


def _missing(obj: object, keys: tuple[str, ...], where: str) -> list[str]:
    if not isinstance(obj, dict):
        return [f"{where} is not an object"]
    return [f"{where} is missing `{key}`" for key in keys if key not in obj]


def _unexpected(obj: object, keys: tuple[str, ...], where: str) -> list[str]:
    """Reject fields no current Rust serializer can have emitted.

    The artifact envelope is a versioned producer contract, not an extensible
    transport object.  Accepting an unknown field here lets a future/foreign
    producer look schema-2-compatible without an explicit schema bump.
    """
    if not isinstance(obj, dict):
        return []
    return [f"{where} has unexpected `{key}`" for key in sorted(set(obj) - set(keys))]


def _non_negative_number(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and value >= 0
        and value != float("inf")
        and value == value
    )


def _validate_rows(rows: list[object], *, dimension: str) -> list[str]:
    """Validate row semantics shared by every BenchArtifactV1 family."""
    reasons: list[str] = []
    scenario_ids: set[str] = set()
    for index, row in enumerate(rows):
        where = f"rows[{index}]"
        reasons.extend(_missing(row, ROW_KEYS, where))
        reasons.extend(_unexpected(row, ROW_KEYS, where))
        if not isinstance(row, dict):
            continue
        scenario_id = row.get("scenario_id")
        if not isinstance(scenario_id, str) or not scenario_id:
            reasons.append(f"{where}.scenario_id is not a non-empty string")
        elif scenario_id in scenario_ids:
            reasons.append(f"{where}.scenario_id {scenario_id!r} is duplicated")
        else:
            scenario_ids.add(scenario_id)
        for key in ("route_family", "syntax", "result_shape"):
            if not isinstance(row.get(key), str) or not row[key]:
                reasons.append(f"{where}.{key} is not a non-empty string")
        for key in ("error_count", "timeout_count"):
            value = row.get(key)
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                reasons.append(f"{where}.{key} is not a non-negative integer")
        result_count = row.get("result_count")
        if result_count is not None and (
            not isinstance(result_count, int) or isinstance(result_count, bool) or result_count < 0
        ):
            reasons.append(f"{where}.result_count is not a non-negative integer or null")
        typed_error = row.get("typed_error_code")
        if typed_error is not None and (not isinstance(typed_error, str) or not typed_error):
            reasons.append(f"{where}.typed_error_code is not a non-empty string or null")
        engines = row.get("engine_touched")
        if not isinstance(engines, list) or not all(
            isinstance(engine, str) and engine for engine in engines
        ):
            reasons.append(f"{where}.engine_touched is not an array of non-empty strings")
        qps = row.get("qps")
        if qps is not None and not _non_negative_number(qps):
            reasons.append(f"{where}.qps is not a finite non-negative number or null")
        early_stop = row.get("early_stop_reason")
        if early_stop is not None and (not isinstance(early_stop, str) or not early_stop):
            reasons.append(f"{where}.early_stop_reason is not a non-empty string or null")
        latency = row.get("latency")
        if early_stop is not None:
            if latency is not None:
                reasons.append(f"{where}.latency must be null when early_stop_reason is set")
            continue
        if latency is None and dimension in UNTIMED_DIMENSIONS:
            continue
        if not isinstance(latency, dict):
            reasons.append(f"{where}.latency is not an object for a measured row")
            continue
        reasons.extend(_missing(latency, LATENCY_KEYS, f"{where}.latency"))
        reasons.extend(_unexpected(latency, LATENCY_KEYS, f"{where}.latency"))
        percentiles: list[float] = []
        for key in ("p50_ms", "p95_ms", "p99_ms"):
            value = latency.get(key)
            if not _non_negative_number(value):
                reasons.append(f"{where}.latency.{key} is not a finite non-negative number")
            else:
                percentiles.append(float(value))
        if len(percentiles) == 3 and not (percentiles[0] <= percentiles[1] <= percentiles[2]):
            reasons.append(f"{where}.latency percentiles are not ordered p50 <= p95 <= p99")
        samples = latency.get("samples")
        if not isinstance(samples, int) or isinstance(samples, bool) or samples < 1:
            reasons.append(f"{where}.latency.samples is not a positive integer")
    return reasons


def check_envelope(
    payload: object,
    *,
    dimension: str,
    head: str | None,
    host_policy: str = "any",
) -> list[str]:
    """Every reason `payload` is not an acceptable artifact for `dimension`.

    `head` is the checkout head a fresh artifact must match; `None` for a
    baseline, which must still carry a full head.
    """
    reasons = _missing(payload, ENVELOPE_KEYS, "envelope")
    reasons.extend(_unexpected(payload, ENVELOPE_KEYS, "envelope"))
    if not isinstance(payload, dict):
        return reasons
    schema = payload.get("schema_version")
    if type(schema) is not int or schema != CURRENT_SCHEMA_VERSION:
        reasons.append(
            f"schema_version {schema!r} is not {CURRENT_SCHEMA_VERSION}; "
            "re-capture the artifact with the current rail"
        )
        return reasons
    if payload.get("dimension") != dimension:
        reasons.append(f"dimension {payload.get('dimension')!r} is not {dimension!r}")
    if payload.get("mode") not in MODES:
        reasons.append(f"mode {payload.get('mode')!r} is not one of {MODES}")
    concurrency = payload.get("concurrency")
    if not isinstance(concurrency, int) or isinstance(concurrency, bool) or concurrency < 1:
        reasons.append(f"concurrency {concurrency!r} is not a positive integer")

    provenance = payload.get("provenance")
    reasons.extend(_missing(provenance, PROVENANCE_KEYS, "provenance"))
    reasons.extend(_unexpected(provenance, PROVENANCE_KEYS, "provenance"))
    if isinstance(provenance, dict):
        git_head = provenance.get("git_head")
        if not isinstance(git_head, str) or not FULL_HEAD_RE.match(git_head):
            reasons.append(f"git_head {git_head!r} is not 40 lowercase hex characters")
        elif head is not None and git_head != head:
            reasons.append(f"git_head {git_head} is not HEAD {head}: stale artifact")
        for key in ("corpus_digest", "config_digest"):
            digest = provenance.get(key)
            if not isinstance(digest, str) or not DIGEST_RE.match(digest):
                reasons.append(f"{key} {digest!r} is not a sha256: digest")
        model_revision = provenance.get("model_revision")
        if model_revision is not None and (
            not isinstance(model_revision, str) or not model_revision
        ):
            reasons.append(f"model_revision {model_revision!r} is neither null nor a name")

    host = payload.get("host")
    reasons.extend(_missing(host, HOST_KEYS, "host"))
    reasons.extend(_unexpected(host, HOST_KEYS, "host"))
    if isinstance(host, dict):
        for key in ("cpu_count", "mem_bytes"):
            value = host.get(key)
            if not isinstance(value, int) or isinstance(value, bool) or value < 1:
                reasons.append(f"host.{key} {value!r} is not a positive integer")
        for key in ("os", "arch"):
            value = host.get(key)
            if not isinstance(value, str) or not value:
                reasons.append(f"host.{key} {value!r} is not a name")
        hostname_hash = host.get("hostname_hash")
        if not isinstance(hostname_hash, str) or not DIGEST_RE.match(hostname_hash):
            reasons.append(f"host.hostname_hash {hostname_hash!r} is not a sha256: digest")
        if host_policy == "canonical-linux" and host.get("os") != "linux":
            reasons.append("canonical-linux artifact was not measured on Linux")

    resources = payload.get("resources")
    reasons.extend(_missing(resources, RESOURCE_KEYS, "resources"))
    reasons.extend(_unexpected(resources, RESOURCE_KEYS, "resources"))
    if isinstance(resources, dict):
        peak = resources.get("peak_rss_bytes")
        if not isinstance(peak, int) or isinstance(peak, bool) or peak < 1:
            reasons.append(f"resources.peak_rss_bytes {peak!r} is not a positive integer")

    phases = payload.get("phases")
    reasons.extend(_missing(phases, PHASE_KEYS, "phases"))
    reasons.extend(_unexpected(phases, PHASE_KEYS, "phases"))
    if isinstance(phases, dict):
        for key in PHASE_KEYS:
            value = phases.get(key)
            if value is not None and not _non_negative_number(value):
                reasons.append(f"phases.{key} is not a finite non-negative number or null")

    amplification = payload.get("disk_amplification")
    if amplification is not None:
        reasons.extend(_missing(amplification, DISK_AMPLIFICATION_KEYS, "disk_amplification"))
        reasons.extend(_unexpected(amplification, DISK_AMPLIFICATION_KEYS, "disk_amplification"))
        if isinstance(amplification, dict):
            bytes_written = amplification.get("bytes_written")
            changed_bytes = amplification.get("changed_bytes")
            for key, value in (("bytes_written", bytes_written), ("changed_bytes", changed_bytes)):
                if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                    reasons.append(f"disk_amplification.{key} is not a non-negative integer")
            ratio = amplification.get("ratio")
            if changed_bytes == 0:
                if ratio is not None:
                    reasons.append(
                        "disk_amplification.ratio must be null when changed_bytes is zero"
                    )
            elif isinstance(bytes_written, int) and isinstance(changed_bytes, int):
                expected_ratio = bytes_written / changed_bytes
                if (
                    not isinstance(ratio, (int, float))
                    or isinstance(ratio, bool)
                    or ratio != expected_ratio
                ):
                    reasons.append(
                        "disk_amplification.ratio does not equal bytes_written / changed_bytes"
                    )

    if not isinstance(payload.get("detail"), dict):
        reasons.append("detail is not an object")
    elif dimension == "relevance":
        detail = payload["detail"]
        model_revision = provenance.get("model_revision") if isinstance(provenance, dict) else None
        quality = detail.get("semantic_quality")
        if model_revision != POTION_CODE_MODEL_REVISION:
            reasons.append("relevance: canonical artifact requires potion-code model provenance")
        if (
            not isinstance(quality, dict)
            or quality.get("case_count") != 12
            or type(quality.get("passed")) is not bool
        ):
            reasons.append(
                "relevance: canonical artifact requires 12 judged paraphrase cases and a verdict"
            )

    rows = payload.get("rows")
    if not isinstance(rows, list):
        reasons.append("rows is not an array")
    else:
        if not rows:
            reasons.append("rows is empty: nothing was measured")
        reasons.extend(_validate_rows(rows, dimension=dimension))
        if dimension == "relevance":
            paraphrase_ids = [
                row.get("scenario_id")
                for row in rows
                if isinstance(row, dict)
                and isinstance(row.get("scenario_id"), str)
                and row["scenario_id"].startswith("relevance.semantic.sem.")
                and row["scenario_id"].endswith(".paraphrase")
                and row.get("route_family") == "semantic"
            ]
            if len(paraphrase_ids) != 12 or len(set(paraphrase_ids)) != 12:
                reasons.append("relevance: canonical artifact requires 12 distinct paraphrase rows")
    return reasons


def parse_artifact_bytes(raw: bytes) -> object:
    """Parse native evidence without last-key-wins or non-finite defaults."""

    def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate native JSON key: {key}")
            result[key] = value
        return result

    def reject_constant(value: str) -> None:
        raise ValueError(f"non-finite native JSON value: {value}")

    return json.loads(raw, object_pairs_hook=unique_object, parse_constant=reject_constant)


def check_artifact(
    payload: object,
    *,
    dimension: str,
    head: str | None,
    require: bool,
    manifest: dict[str, object] = MANIFEST,
    artifact_path: Path | None = None,
) -> list[str]:
    """One family policy for scanning, promotion and captured-raw replay."""
    family = manifest["families"][dimension]
    reasons = check_envelope(
        payload, dimension=dimension, head=head, host_policy=family["host_policy"]
    )
    if isinstance(payload, dict) and dimension == "concurrency":
        reasons.extend(validate_concurrency(payload, family["minimum_samples"], artifact_path))
    if not require or not isinstance(payload, dict):
        return reasons
    rows = payload.get("rows")
    minimum = family["minimum_samples"]
    if minimum is not None and isinstance(rows, list):
        for index, row in enumerate(rows):
            latency = row.get("latency") if isinstance(row, dict) else None
            if (
                not isinstance(latency, dict)
                or type(latency.get("samples")) is not int
                or latency["samples"] < minimum
            ):
                reasons.append(f"{dimension}: rows[{index}] needs at least {minimum} samples")
    detail = payload.get("detail")
    if family["requires_verdict"] and (
        not isinstance(detail, dict) or detail.get("passed") is not True
    ):
        reasons.append(f"{dimension}: required rail verdict is not true")
    if dimension == "open-loop" and (
        not isinstance(detail, dict)
        or detail.get("arrival_model") != "seeded_poisson"
        or type(detail.get("duration_ms")) is not int
        or detail["duration_ms"] < 10_000
        or not isinstance(detail.get("points"), list)
        or len(detail["points"]) < 4
    ):
        reasons.append(
            "open-loop: authority needs seeded_poisson, >=10 s and >=4 offered-load points"
        )
    if isinstance(rows, list) and any(
        isinstance(row, dict) and row.get("early_stop_reason") is not None for row in rows
    ):
        reasons.append(f"{dimension}: required measurement contains an early stop")
    return reasons


def expand(repo_root: Path, pattern: str) -> list[Path]:
    if any(char in pattern for char in "*?["):
        return sorted(repo_root.glob(pattern))
    path = repo_root / pattern
    return [path] if path.exists() else []


def check_families(
    repo_root: Path,
    families: tuple[tuple[str, str], ...],
    *,
    head: str | None,
    require: bool,
    manifest: dict[str, object] = MANIFEST,
) -> tuple[list[Refusal], list[Path], list[str]]:
    """Refusals, the artifacts checked, and the families with no artifact."""
    refusals: list[Refusal] = []
    checked: list[Path] = []
    absent: list[str] = []
    for dimension, pattern in families:
        paths = expand(repo_root, pattern)
        if not paths:
            absent.append(dimension)
            if require:
                refusals.append(
                    Refusal(repo_root / pattern, f"{dimension}: no artifact (required)")
                )
            continue
        if dimension == "concurrency" and require:
            expected = {f"summary-c{count}.json" for count in CONCURRENCY_COUNTS}
            actual = {path.name for path in paths}
            for missing in sorted(expected - actual):
                refusals.append(
                    Refusal(
                        repo_root / "artifacts/search-quality/concurrency/latest" / missing,
                        "concurrency: required client-count artifact missing",
                    )
                )
        for path in paths:
            checked.append(path)
            try:
                payload = parse_artifact_bytes(path.read_bytes())
            except (OSError, ValueError) as exc:
                refusals.append(Refusal(path, f"unreadable: {exc}"))
                continue
            for reason in check_artifact(
                payload,
                dimension=dimension,
                head=head,
                require=require,
                manifest=manifest,
                artifact_path=path,
            ):
                refusals.append(Refusal(path, reason))
    return refusals, checked, absent


def select_families(
    profile: str | None,
    *,
    fresh: tuple[tuple[str, str], ...] = FRESH_FAMILIES,
    profiles: dict[str, tuple[str, ...]] = FAMILY_PROFILES,
) -> tuple[tuple[str, str], ...]:
    """Resolve an explicit evidence profile without changing family ownership."""
    if profile is None:
        return fresh
    names = profiles[profile]
    by_name = dict(fresh)
    return tuple((name, by_name[name]) for name in names)


def control_plane(
    repo_root: Path,
) -> tuple[
    dict[str, object],
    tuple[tuple[str, str], ...],
    tuple[tuple[str, str], ...],
    dict[str, tuple[str, ...]],
]:
    """Load the manifest belonging to the checkout being gated."""
    manifest = load_manifest(repo_root / "tools" / "benchmark" / "registry.toml")
    return (
        manifest,
        fresh_families(manifest),
        baseline_families(manifest),
        profile_families(manifest),
    )


def parse_args(
    argv: list[str] | None,
    *,
    profiles: dict[str, tuple[str, ...]] = FAMILY_PROFILES,
) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="checkout to gate (default: this script's repository)",
    )
    parser.add_argument(
        "--profile",
        choices=sorted(profiles),
        help="require/check only one named evidence profile (default: every fresh family)",
    )
    parser.add_argument(
        "--head",
        default=None,
        help="the head fresh artifacts must match (default: `git rev-parse HEAD` of --repo-root)",
    )
    parser.add_argument(
        "--require",
        action="store_true",
        help="fail when a fresh family has no artifact (the perf evidence gate)",
    )
    parser.add_argument(
        "--require-clean-worktree",
        action="store_true",
        help="refuse qualification unless --repo-root has no tracked or untracked source changes",
    )
    parser.add_argument(
        "--skip-baselines",
        action="store_true",
        help="do not check the committed baselines",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    bootstrap = argparse.ArgumentParser(add_help=False)
    bootstrap.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    bootstrap_args, _ = bootstrap.parse_known_args(argv)
    repo_root: Path = bootstrap_args.repo_root.resolve()
    try:
        manifest, fresh, baselines, profiles = control_plane(repo_root)
    except ManifestError as exc:
        print(f"ERROR: invalid benchmark manifest: {exc}", file=sys.stderr)
        return 2
    args = parse_args(argv, profiles=profiles)
    repo_root = args.repo_root.resolve()
    try:
        if args.require_clean_worktree:
            require_clean_worktree(repo_root)
        head = args.head if args.head is not None else resolve_head(repo_root)
    except RuntimeError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    if not FULL_HEAD_RE.match(head):
        print(f"ERROR: --head {head!r} is not 40 lowercase hex characters", file=sys.stderr)
        return 2

    selected_families = select_families(args.profile, fresh=fresh, profiles=profiles)
    refusals, checked, absent = check_families(
        repo_root, selected_families, head=head, require=args.require, manifest=manifest
    )
    if not args.skip_baselines:
        markers = {
            (repo_root / relative).parent / ".dsl-admission-pending"
            for _name, relative in baselines
        }
        for marker in sorted(markers):
            try:
                marker.lstat()
            except FileNotFoundError:
                pass
            except OSError as exc:
                refusals.append(f"cannot inspect DSL baseline admission marker {marker}: {exc}")
            else:
                refusals.append(f"DSL baseline admission is incomplete: {marker}")
        baseline_refusals, baseline_checked, baseline_absent = check_families(
            repo_root, baselines, head=None, require=False, manifest=manifest
        )
        refusals.extend(baseline_refusals)
        checked.extend(baseline_checked)
        absent.extend(f"baseline:{name}" for name in baseline_absent)

    for path in checked:
        print(f"checked {path.relative_to(repo_root) if path.is_relative_to(repo_root) else path}")
    for name in absent:
        print(f"absent  {name}")
    if refusals:
        print()
        for refusal in refusals:
            print(f"REFUSED {refusal}", file=sys.stderr)
        print(
            f"FAIL: {len(refusals)} refusal(s) across {len(checked)} artifact(s) at HEAD {head}",
            file=sys.stderr,
        )
        return 1
    print(
        f"OK: {len(checked)} benchmark artifact(s) attributed to HEAD {head}; {len(absent)} absent"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
