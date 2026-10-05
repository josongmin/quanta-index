"""Replay opt-in scale lifecycle cost markers against one checked scale artifact.

Run a matching `scale_matrix` binary with QUANTA_INDEX_CAUSAL_PROFILE_V1=1,
capture stderr outside the checkout, then pass its summary and binary here.
The output is diagnostic. Timers are userspace call envelopes; /proc/self/io
write_bytes is kernel process accounting, not device-completed physical I/O.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from tools.benchmark.retrieval.conditional_proof import canonical, load, sha
from tools.benchmark.retrieval.finite_json import is_finite_json_number

PREFIX = "QI_CAUSAL_V1 "
SYNC_LABELS = frozenset(
    {
        "atomic_file",
        "atomic_parent",
        "atomic_at_file",
        "atomic_at_parent",
        "generation_directory",
        "coverage_orphan_directory",
        "history_manifest_file",
        "sealed_leftover_directory",
        "overlay_directory",
        "text_authority_directory",
        "history_layout_directory",
    }
)
KINDS = frozenset(
    {
        "phase_start",
        "phase_end",
        "sync",
        "exact_live_token_scan",
        "bm25_census_build",
        "bm25_boundary",
        "bm25_live_build",
    }
)
BM25_BOUNDARY_LABELS = frozenset({"base_read", "encode"})
BM25_BUILD_COUNTERS = frozenset(
    {
        "elapsed_ns",
        "reused_segments",
        "changed_segments",
        "new_segments",
        "mask_docs",
        "new_census_docs",
        "newly_dead_docs",
        "logical_census_bytes",
        "changed_ns",
        "new_ns",
        "death_ns",
        "correction_keys",
        "segment_fanout",
        "retained_valid",
        "retained_estimate_bytes",
    }
)
TIER_SHAPE = {
    "small": (1, 16),
    "medium": (4, 64),
    "large": (16, 256),
    "xlarge": (64, 512),
}
SMALL_PHASES = frozenset(
    {
        "full_ingest_seal",
        "full_activate",
        "delta_ingest_seal",
        "delta_activate",
        "noop_seal",
        "noop_activate",
    }
)
LARGE_PHASES = frozenset(
    {
        "full_ingest",
        "full_seal",
        "full_activate",
        "delta_ingest_seal",
        "delta_activate",
        "noop_seal",
        "noop_activate",
        "delete_seal",
        "delete_activate",
        "same_process_reopen",
    }
)
DEFAULT_TIMEOUT_MS = 30_000
DEFAULT_HISTORY_MAX_BYTES = 16 * 1024 * 1024
HISTORY_MAX_TOTAL_BYTES = 256 * 1024 * 1024
HISTORY_MAX_REVISION_PAIRS = 128


def _exact_int(value: object, expected: int, label: str) -> None:
    if type(value) is not int or value != expected:
        raise ValueError(f"{label} does not match declared scale input")


def _declared_inputs(
    manifest_raw: bytes,
    tier: dict,
    *,
    expected_tier: str,
    expected_seed: int,
    requested_client_timeout_ms: int | None,
    requested_history_max_bytes: int | None,
    requested_history_max_total_bytes: int | None,
) -> dict:
    if (
        expected_tier not in TIER_SHAPE
        or type(expected_seed) is not int
        or not 0 <= expected_seed < 1 << 64
    ):
        raise ValueError("invalid declared scale tier or seed")
    if requested_client_timeout_ms is not None and (
        type(requested_client_timeout_ms) is not int
        or not 1 <= requested_client_timeout_ms <= 600_000
    ):
        raise ValueError("invalid declared client timeout")
    if requested_history_max_bytes is not None and (
        type(requested_history_max_bytes) is not int
        or not 1 <= requested_history_max_bytes <= (1 << 64) - 1
    ):
        raise ValueError("invalid declared history limit")
    if requested_history_max_total_bytes is not None and (
        type(requested_history_max_total_bytes) is not int
        or not 1 <= requested_history_max_total_bytes <= (1 << 64) - 1
    ):
        raise ValueError("invalid declared total history limit")
    if requested_history_max_total_bytes is not None and requested_history_max_bytes is None:
        raise ValueError("total history override requires an explicit pair bound")
    effective_total = requested_history_max_total_bytes or HISTORY_MAX_TOTAL_BYTES
    effective_pair = requested_history_max_bytes or DEFAULT_HISTORY_MAX_BYTES
    if effective_pair > effective_total:
        raise ValueError("pair history bound exceeds total history bound")
    policy_id = (
        "explicit-pair-total-diagnostic-v1"
        if requested_history_max_total_bytes is not None
        else "explicit-pair-default-total-v1"
        if requested_history_max_bytes is not None
        else "harness-default-v1"
    )
    manifest = load(manifest_raw)
    if (
        not isinstance(manifest, dict)
        or manifest.get("kind") != "quanta-index-scale-tier-manifest"
        or type(manifest.get("manifest_schema_version")) is not int
        or manifest["manifest_schema_version"] != 2
        or manifest.get("dimension") != "scale"
        or manifest.get("query_token") != "scale_needle_token"
        or not isinstance(manifest.get("tiers"), list)
        or len(manifest["tiers"]) != len(TIER_SHAPE)
    ):
        raise ValueError("scale tier manifest contract is invalid")
    for row, (name, (repos, files_per_repo)) in zip(
        manifest["tiers"], TIER_SHAPE.items(), strict=True
    ):
        if not isinstance(row, dict) or row.get("tier") != name:
            raise ValueError("scale tier manifest order or name changed")
        for key, expected in (
            ("repo_count", repos),
            ("files_per_repo", files_per_repo),
            ("total_files", repos * files_per_repo),
            ("source_repo_count", repos),
        ):
            _exact_int(row.get(key), expected, f"manifest {name}.{key}")
        if row.get("default_run") is not (name == "small") or row.get("selectable") is not True:
            raise ValueError("scale tier manifest selection policy changed")

    repos, files_per_repo = TIER_SHAPE[expected_tier]
    if tier.get("tier") != expected_tier:
        raise ValueError("measured tier differs from declared command")
    _exact_int(tier.get("seed"), expected_seed, "measured seed")
    _exact_int(tier.get("file_count"), repos * files_per_repo, "measured file count")
    _exact_int(tier.get("source_repo_count"), repos, "measured source repo count")
    policy = {
        "requested_client_request_timeout_ms": requested_client_timeout_ms,
        "client_request_timeout_ms": requested_client_timeout_ms or DEFAULT_TIMEOUT_MS,
        "requested_history_max_bytes": requested_history_max_bytes,
        "history_max_bytes": effective_pair,
        "requested_history_max_total_bytes": requested_history_max_total_bytes,
        "history_policy_id": policy_id,
        "history_max_generations": 2,
        "history_max_revision_pairs": HISTORY_MAX_REVISION_PAIRS,
        "history_max_total_bytes": effective_total,
    }
    for key, expected in policy.items():
        if expected is None:
            if key not in tier or tier[key] is not None:
                raise ValueError(f"{key} does not match declared scale input")
        elif isinstance(expected, str):
            if type(tier.get(key)) is not str or tier[key] != expected:
                raise ValueError(f"{key} does not match declared scale input")
        else:
            _exact_int(tier.get(key), expected, key)
    return policy


def _fields(line: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for part in line[len(PREFIX) :].split():
        key, separator, value = part.partition("=")
        if not separator or not key or not value or key in result:
            raise ValueError("malformed or duplicated causal marker field")
        result[key] = value
    if result.get("kind") not in KINDS:
        raise ValueError("unknown causal marker kind")
    return result


def _natural(row: dict[str, str], name: str) -> int:
    value = row[name]
    if not value.isascii() or not value.isdecimal() or len(value) > 20:
        raise ValueError(f"invalid {name} in causal marker")
    parsed = int(value)
    if parsed > (1 << 64) - 1:
        raise ValueError(f"{name} exceeds u64 in causal marker")
    return parsed


def _check_fields(row: dict[str, str], expected: set[str]) -> None:
    if set(row) != expected:
        raise ValueError(f"wrong causal marker fields for {row['kind']}")


def _add_u64(counter: dict[str, int], key: str, amount: int) -> None:
    next_value = counter[key] + amount
    if next_value > (1 << 64) - 1:
        raise ValueError(f"aggregated {key} exceeds u64 in causal marker")
    counter[key] = next_value


def parse_trace(raw: bytes, expected_phases: set[str]) -> dict:
    text = raw.decode("utf-8")
    phases: dict[str, dict] = {}
    unattributed = {"sync_calls": 0, "exact_live_token_scans": 0, "bm25_events": 0}
    active: str | None = None
    for line in text.splitlines():
        if not line.startswith(PREFIX):
            continue
        row = _fields(line)
        kind = row["kind"]
        if kind == "phase_start":
            _check_fields(row, {"kind", "name"})
            name = row["name"]
            if name not in expected_phases or name in phases or active is not None:
                raise ValueError("unexpected, repeated or nested scale phase")
            phases[name] = {
                "status": "running",
                "sync": {},
                "exact_live_token_scan": {
                    "calls": 0,
                    "elapsed_ns": 0,
                    "terms": 0,
                    "postings": 0,
                    "live_postings": 0,
                    "live_tokens": 0,
                },
                "bm25_census_build": {"calls": 0, "elapsed_ns": 0, "docs": 0, "encoded_bytes": 0},
                "bm25_boundary": {},
                "bm25_live_build": [],
            }
            active = name
        elif kind == "phase_end":
            _check_fields(row, {"kind", "name", "ok"})
            if row["name"] != active or row["ok"] not in ("0", "1"):
                raise ValueError("unpaired or malformed scale phase end")
            phases[active]["status"] = "ok" if row["ok"] == "1" else "failed"
            active = None
        elif kind == "sync":
            _check_fields(row, {"kind", "label", "ok", "elapsed_ns"})
            if row["label"] not in SYNC_LABELS or row["ok"] not in ("0", "1"):
                raise ValueError("invalid lexical sync marker")
            if row["ok"] == "0":
                raise ValueError("failed lexical sync in a successful scale artifact")
            elapsed = _natural(row, "elapsed_ns")
            if active is None:
                unattributed["sync_calls"] += 1
                continue
            counter = phases[active]["sync"].setdefault(
                row["label"], {"calls": 0, "failed_calls": 0, "elapsed_ns": 0}
            )
            counter["calls"] += 1
            counter["failed_calls"] += row["ok"] == "0"
            counter["elapsed_ns"] += elapsed
        elif kind == "bm25_census_build":
            if row.get("ok") == "0":
                _check_fields(row, {"kind", "ok", "reason"})
                if row["reason"] != "counter_overflow":
                    raise ValueError("invalid BM25 census failure reason")
                raise ValueError("BM25 census observation overflowed")
            _check_fields(row, {"kind", "ok", "elapsed_ns", "docs", "encoded_bytes"})
            if row["ok"] != "1":
                raise ValueError("failed BM25 census build marker")
            counts = {key: _natural(row, key) for key in ("elapsed_ns", "docs", "encoded_bytes")}
            if counts["docs"] == 0 and counts["encoded_bytes"] != 0:
                raise ValueError("BM25 census bytes without documents")
            if active is None:
                unattributed["bm25_events"] += 1
                continue
            counter = phases[active]["bm25_census_build"]
            _add_u64(counter, "calls", 1)
            for key, value in counts.items():
                _add_u64(counter, key, value)
        elif kind == "bm25_boundary":
            if row.get("ok") == "0":
                _check_fields(row, {"kind", "label", "ok", "reason"})
                if row["label"] not in BM25_BOUNDARY_LABELS or row["reason"] != "counter_overflow":
                    raise ValueError("invalid BM25 boundary failure")
                raise ValueError("BM25 boundary observation overflowed")
            _check_fields(
                row, {"kind", "label", "ok", "elapsed_ns", "logical_bytes", "index_files"}
            )
            if row["ok"] != "1" or row["label"] not in BM25_BOUNDARY_LABELS:
                raise ValueError("invalid BM25 boundary marker")
            counts = {
                key: _natural(row, key) for key in ("elapsed_ns", "logical_bytes", "index_files")
            }
            if active is None:
                unattributed["bm25_events"] += 1
                continue
            counter = phases[active]["bm25_boundary"].setdefault(
                row["label"], {"calls": 0, "elapsed_ns": 0, "logical_bytes": 0, "index_files": 0}
            )
            _add_u64(counter, "calls", 1)
            for key, value in counts.items():
                _add_u64(counter, key, value)
        elif kind == "bm25_live_build":
            if row.get("ok") == "0":
                _check_fields(row, {"kind", "ok", "reason"})
                if row["reason"] not in {"counter_overflow", "retained_estimate_failed"}:
                    raise ValueError("invalid BM25 live build failure reason")
                raise ValueError("BM25 live build observation overflowed")
            _check_fields(row, {"kind", "ok"} | BM25_BUILD_COUNTERS)
            if row["ok"] != "1":
                raise ValueError("failed BM25 live build marker")
            counts = {key: _natural(row, key) for key in BM25_BUILD_COUNTERS}
            if (
                counts["retained_valid"] != 1
                or counts["reused_segments"] + counts["changed_segments"] + counts["new_segments"]
                != counts["segment_fanout"]
                or counts["newly_dead_docs"] > counts["mask_docs"]
                or counts["death_ns"] > counts["changed_ns"]
            ):
                raise ValueError("inconsistent BM25 live build marker")
            # These sibling scopes run synchronously and do not overlap. The
            # death timer is nested within changed_ns and is not added here.
            if counts["changed_ns"] + counts["new_ns"] > counts["elapsed_ns"]:
                raise ValueError("BM25 child call clocks exceed parent")
            if active is None:
                unattributed["bm25_events"] += 1
                continue
            phases[active]["bm25_live_build"].append(counts)
        else:
            _check_fields(
                row,
                {
                    "kind",
                    "ok",
                    "elapsed_ns",
                    "terms",
                    "postings",
                    "live_postings",
                    "live_tokens",
                },
            )
            if row["ok"] != "1":
                raise ValueError("failed exact live token scan marker")
            counts = {key: _natural(row, key) for key in row if key not in ("kind", "ok")}
            if counts["live_postings"] > counts["postings"]:
                raise ValueError("live posting count exceeds scanned postings")
            if active is None:
                unattributed["exact_live_token_scans"] += 1
                continue
            counter = phases[active]["exact_live_token_scan"]
            counter["calls"] += 1
            for key, value in counts.items():
                counter[key] += value
    if active is not None or set(phases) != expected_phases:
        raise ValueError("incomplete scale phase marker coverage")
    if any(phase["status"] != "ok" for phase in phases.values()):
        raise ValueError("failed scale phase marker")
    if not any(phase["sync"] for phase in phases.values()):
        raise ValueError("scale trace has no lexical durability call markers")
    seal_phases = {"full_ingest_seal", "full_seal", "delta_ingest_seal", "noop_seal", "delete_seal"}
    for name, phase in phases.items():
        if name in seal_phases and (
            len(phase["bm25_live_build"]) != 1
            or phase["bm25_boundary"].get("encode", {}).get("calls") != 1
        ):
            raise ValueError(f"{name} lacks one complete BM25 seal observation")
    return {"phases": phases, "unattributed": unattributed}


def replay(
    summary_raw: bytes,
    trace_raw: bytes,
    binary_raw: bytes,
    manifest_raw: bytes,
    *,
    source_revision: str,
    expected_tier: str,
    expected_seed: int,
    requested_client_timeout_ms: int | None,
    requested_history_max_bytes: int | None,
    requested_history_max_total_bytes: int | None = None,
) -> dict:
    summary = load(summary_raw)
    if (
        not isinstance(summary, dict)
        or type(summary.get("schema_version")) is not int
        or summary["schema_version"] != 2
    ):
        raise ValueError("expected one BenchArtifactV1 scale summary")
    detail = summary.get("detail")
    provenance = summary.get("provenance")
    if (
        not isinstance(detail, dict)
        or not isinstance(provenance, dict)
        or detail.get("passed") is not True
        or not isinstance(detail.get("measured_tiers"), list)
        or len(detail["measured_tiers"]) != 1
        or provenance.get("git_head") != source_revision
    ):
        raise ValueError("scale artifact is failed, partial or from another source")
    for digest_key in ("corpus_digest", "config_digest"):
        digest = provenance.get(digest_key)
        if (
            not isinstance(digest, str)
            or not digest.startswith("sha256:")
            or len(digest) != 71
            or any(character not in "0123456789abcdef" for character in digest[7:])
        ):
            raise ValueError(f"scale artifact lacks a bound {digest_key}")
    tier = detail["measured_tiers"][0]
    if not isinstance(tier, dict):
        raise ValueError("scale artifact measured tier is malformed")
    resources = tier.get("phase_resources")
    if (
        tier.get("status") != "measured"
        or not isinstance(resources, dict)
        or not resources
        or any(not isinstance(value, dict) for value in resources.values())
    ):
        raise ValueError("scale artifact lacks measured phase resources")
    policy = _declared_inputs(
        manifest_raw,
        tier,
        expected_tier=expected_tier,
        expected_seed=expected_seed,
        requested_client_timeout_ms=requested_client_timeout_ms,
        requested_history_max_bytes=requested_history_max_bytes,
        requested_history_max_total_bytes=requested_history_max_total_bytes,
    )
    expected_phases = SMALL_PHASES if expected_tier == "small" else LARGE_PHASES
    if set(resources) != expected_phases:
        raise ValueError("scale artifact has incomplete or unexpected lifecycle phases")
    trace = parse_trace(trace_raw, set(expected_phases))
    for name, observation in resources.items():
        io = observation.get("process_write_io")
        reason = observation.get("process_write_io_unavailable_reason")
        if (io is None) == (reason is None):
            raise ValueError(f"{name} process write I/O state is ambiguous")
        if reason is not None and (not isinstance(reason, str) or not reason):
            raise ValueError(f"{name} process write I/O reason is invalid")
        if io is not None and (
            not isinstance(io, dict)
            or set(io) != {"write_bytes", "cancelled_write_bytes", "syscw", "wchar"}
            or any(type(value) is not int for value in io.values())
            or any(not 0 <= io[key] <= (1 << 64) - 1 for key in ("write_bytes", "syscw", "wchar"))
            or not -(1 << 63) <= io["cancelled_write_bytes"] <= (1 << 63) - 1
        ):
            raise ValueError(f"{name} process write I/O counters are invalid")
        trace["phases"][name]["process_write_io"] = io
        trace["phases"][name]["process_write_io_unavailable_reason"] = reason
        for cpu_key in ("cpu_process_user_ms", "cpu_process_system_ms"):
            cpu_value = observation.get(cpu_key)
            if not is_finite_json_number(cpu_value) or cpu_value < 0:
                raise ValueError(f"{name} {cpu_key} is invalid")
            trace["phases"][name][cpu_key] = cpu_value
        span_ms = observation.get("observation_span_ms")
        if not is_finite_json_number(span_ms) or span_ms <= 0:
            raise ValueError(f"{name} has no positive observed wall span")
        phase = trace["phases"][name]
        sync_ns = sum(counter["elapsed_ns"] for counter in phase["sync"].values())
        scan_ns = phase["exact_live_token_scan"]["elapsed_ns"]
        phase["observation_span_ms"] = span_ms
        phase["sync_call_elapsed_ns_sum"] = sync_ns
        phase["exact_scan_elapsed_ns_sum"] = scan_ns
        phase["sync_call_wall_ratio"] = sync_ns / (span_ms * 1_000_000)
        phase["exact_scan_wall_ratio"] = scan_ns / (span_ms * 1_000_000)
    return {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "source_revision": source_revision,
        "binary_sha256": sha(binary_raw),
        "summary_sha256": sha(summary_raw),
        "trace_sha256": sha(trace_raw),
        "corpus_digest": provenance.get("corpus_digest"),
        "config_digest": provenance.get("config_digest"),
        "tier": tier["tier"],
        "seed": tier["seed"],
        "file_count": tier["file_count"],
        "runtime_config": policy,
        "scope": {
            "sync": "all explicit sync_all calls in quanta-index-lexical; dependency and semantic sync excluded",
            "posting": "Tantivy deleted-segment pure-string exact-live-token-count pass only",
            "bm25": "census build and seal call envelopes; logical bytes count encoded payloads already handled by the caller, not physical read/write bytes; changed/death timers are nested",
            "io": "Linux /proc/self/io process counters; not device-completed bytes, fsync latency or phase-exclusive I/O",
            "wall": "opt-in instrumentation perturbs timing; unprofiled matched source run needed for effect",
            "ratios": "sums of call envelopes divided by phase observation span; concurrent work may overlap, so shares are diagnostic and not additive",
        },
        **trace,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--summary", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--trace", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--tier", choices=tuple(TIER_SHAPE), required=True)
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--client-timeout-ms", type=int)
    parser.add_argument("--history-max-bytes", type=int)
    parser.add_argument("--history-max-total-bytes", type=int)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        if len(args.source_revision) != 40 or any(
            c not in "0123456789abcdef" for c in args.source_revision
        ):
            raise ValueError("source revision must be a full lowercase commit SHA")
        result = replay(
            args.summary.read_bytes(),
            args.trace.read_bytes(),
            args.binary.read_bytes(),
            args.manifest.read_bytes(),
            source_revision=args.source_revision,
            expected_tier=args.tier,
            expected_seed=args.seed,
            requested_client_timeout_ms=args.client_timeout_ms,
            requested_history_max_bytes=args.history_max_bytes,
            requested_history_max_total_bytes=args.history_max_total_bytes,
        )
        with args.out.open("xb") as stream:
            stream.write(canonical(result) + b"\n")
    except (ValueError, OSError, KeyError, TypeError) as error:
        parser.exit(2, f"causal cost replay refused: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
