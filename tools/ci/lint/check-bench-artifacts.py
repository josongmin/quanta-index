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

#: The schema every benchmark artifact must carry, mirroring
#: ``BENCH_ARTIFACT_SCHEMA_VERSION`` in ``artifact.rs``.
CURRENT_SCHEMA_VERSION = 2

FULL_HEAD_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")

#: Fresh-run artifact families (gitignored ``artifacts/``): held to HEAD.
FRESH_FAMILIES: tuple[tuple[str, str], ...] = (
    ("dsl-warm", "artifacts/dsl-bench/warm-matrix.json"),
    ("dsl-cold", "artifacts/dsl-bench/cold-matrix.json"),
    ("scale", "artifacts/search-quality/scale/latest/summary.json"),
    ("tail", "artifacts/search-quality/tail/latest/summary.json"),
    ("ann", "artifacts/search-quality/ann/latest/summary.json"),
    ("relevance", "artifacts/search-quality/relevance/latest/summary.json"),
    ("relevance-openai-ab", "artifacts/search-quality/relevance/openai-ab/latest/summary.json"),
    ("concurrency", "artifacts/search-quality/concurrency/latest/summary-c*.json"),
    ("scan-vs-index", "artifacts/experiments/scan-vs-index/*.json"),
)

#: Committed baselines: held to the shape and a full head, not HEAD equality.
BASELINE_FAMILIES: tuple[tuple[str, str], ...] = (
    ("dsl-warm", "tools/benchmark/baselines/warm-matrix.json"),
    ("dsl-cold", "tools/benchmark/baselines/cold-matrix.json"),
)

# Named evidence sets prevent a small rail from accidentally becoming a proxy
# for every benchmark family.  `--require --profile dsl-authority` is the
# scheduled DSL gate; `quality-full` is the complete locally runnable quality
# evidence set.  A caller that omits --profile retains the exhaustive audit.
FAMILY_PROFILES: dict[str, tuple[str, ...]] = {
    "dsl-authority": ("dsl-warm", "dsl-cold"),
    "quality-core": ("relevance", "scale", "tail"),
    "quality-full": ("relevance", "scale", "tail", "ann", "concurrency"),
    "semantic-ab": ("relevance-openai-ab",),
    "experiments": ("scan-vs-index",),
}

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


def _missing(obj: object, keys: tuple[str, ...], where: str) -> list[str]:
    if not isinstance(obj, dict):
        return [f"{where} is not an object"]
    return [f"{where} is missing `{key}`" for key in keys if key not in obj]


def check_envelope(
    payload: object,
    *,
    dimension: str,
    head: str | None,
) -> list[str]:
    """Every reason `payload` is not an acceptable artifact for `dimension`.

    `head` is the checkout head a fresh artifact must match; `None` for a
    baseline, which must still carry a full head.
    """
    reasons = _missing(payload, ENVELOPE_KEYS, "envelope")
    if not isinstance(payload, dict):
        return reasons
    schema = payload.get("schema_version")
    if schema != CURRENT_SCHEMA_VERSION:
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
    if isinstance(host, dict):
        for key in ("cpu_count", "mem_bytes"):
            value = host.get(key)
            if not isinstance(value, int) or isinstance(value, bool) or value < 1:
                reasons.append(f"host.{key} {value!r} is not a positive integer")
        for key in ("os", "arch", "hostname_hash"):
            value = host.get(key)
            if not isinstance(value, str) or not value:
                reasons.append(f"host.{key} {value!r} is not a name")

    resources = payload.get("resources")
    reasons.extend(_missing(resources, RESOURCE_KEYS, "resources"))
    if isinstance(resources, dict):
        peak = resources.get("peak_rss_bytes")
        if not isinstance(peak, int) or isinstance(peak, bool) or peak < 1:
            reasons.append(f"resources.peak_rss_bytes {peak!r} is not a positive integer")

    reasons.extend(_missing(payload.get("phases"), PHASE_KEYS, "phases"))

    rows = payload.get("rows")
    if not isinstance(rows, list):
        reasons.append("rows is not an array")
    else:
        if not rows:
            reasons.append("rows is empty: nothing was measured")
        for index, row in enumerate(rows):
            reasons.extend(_missing(row, ROW_KEYS, f"rows[{index}]"))
            if isinstance(row, dict) and row.get("latency") is not None:
                reasons.extend(_missing(row["latency"], LATENCY_KEYS, f"rows[{index}].latency"))
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
        for path in paths:
            checked.append(path)
            try:
                payload = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as exc:
                refusals.append(Refusal(path, f"unreadable: {exc}"))
                continue
            for reason in check_envelope(payload, dimension=dimension, head=head):
                refusals.append(Refusal(path, reason))
    return refusals, checked, absent


def select_families(profile: str | None) -> tuple[tuple[str, str], ...]:
    """Resolve an explicit evidence profile without changing family ownership."""
    if profile is None:
        return FRESH_FAMILIES
    names = FAMILY_PROFILES[profile]
    by_name = dict(FRESH_FAMILIES)
    return tuple((name, by_name[name]) for name in names)


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="checkout to gate (default: this script's repository)",
    )
    parser.add_argument(
        "--profile",
        choices=sorted(FAMILY_PROFILES),
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
        "--skip-baselines",
        action="store_true",
        help="do not check the committed baselines",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    repo_root: Path = args.repo_root.resolve()
    try:
        head = args.head if args.head is not None else resolve_head(repo_root)
    except RuntimeError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    if not FULL_HEAD_RE.match(head):
        print(f"ERROR: --head {head!r} is not 40 lowercase hex characters", file=sys.stderr)
        return 2

    fresh_families = select_families(args.profile)
    refusals, checked, absent = check_families(
        repo_root, fresh_families, head=head, require=args.require
    )
    if not args.skip_baselines:
        baseline_refusals, baseline_checked, baseline_absent = check_families(
            repo_root, BASELINE_FAMILIES, head=None, require=False
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
