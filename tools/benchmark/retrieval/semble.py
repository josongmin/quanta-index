#!/usr/bin/env python3
"""Pinned Semble same-corpus comparison adapter (RB-04).

Runs a pinned Semble 0.6.0 install (an outside-the-checkout virtualenv)
against the exact admitted file universe and blind query pack, then
normalizes its native results into a v5 runner record for the single
Quanta-owned evaluator. Semble ranking is never reimplemented here.

Layout contract (all outside the source checkout):
  <output-root>/
    corpus/                 # isolated corpus: admitted files only
    worker.py               # exact spawned worker (auditable)
    native.json             # Semble-native results + timings + observed files
    mapping-proof.json      # path map + both-side path+SHA diff
    record.json             # current runner record (schema v5)
    lockfile.txt            # external digest-pinned exact freeze copy

A common-universe pair requires a clean mapping proof: every admitted file
observed in Semble's indexed chunks with matching bytes. Anything else is a
typed refusal, never a silent subset.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path

try:
    from tools.benchmark.retrieval.finite_json import is_finite_json_number
    from tools.benchmark.retrieval.retrieval_contract import (
        COMPLETED_OUTPUT_VALIDATION,
        TOKEN_RE,
        TOKENIZER,
        TOKENIZER_BUDGET_VERSION,
        canonical,
        completed_output_sha256,
        digest,
        validate_comparison_contract,
        verify_repo,
    )
except ImportError:  # direct script invocation: import the sibling module
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from finite_json import is_finite_json_number  # noqa: E402
    from retrieval_contract import (  # noqa: E402
        COMPLETED_OUTPUT_VALIDATION,
        TOKEN_RE,
        TOKENIZER,
        TOKENIZER_BUDGET_VERSION,
        canonical,
        completed_output_sha256,
        digest,
        validate_comparison_contract,
        verify_repo,
    )

SEMBLE_PINNED_VERSION = "0.6.0"
DEFAULT_MODEL_ID = "minishlab/potion-code-16M-v2"
QUERY_TIMING_BOUNDARY = "request_construction_to_normalized_response"
QUERY_TIMING_CLOCK = "capture_relative_monotonic_ns"


def execution_profile(mode: str, alpha: float | None) -> dict:
    if not isinstance(mode, str):
        raise AdapterError("Semble execution profile mode must be a string")
    fixed = {
        "native-default": ("semble-native-default-v1", None, "upstream-content-default"),
        "lexical-only": ("semble-lexical-only-v1", None, "not_applicable"),
        "lexical-file": ("semble-lexical-file-v1", None, "not_applicable"),
        "semantic-only": ("semble-semantic-only-v1", None, "not_applicable"),
    }
    if mode == "hybrid-no-rerank":
        if not is_finite_json_number(alpha) or not 0 <= alpha <= 1:
            raise AdapterError("hybrid-no-rerank requires finite alpha in [0, 1]")
        return {
            "profile_id": "semble-hybrid-no-rerank-v1",
            "mode": mode,
            "alpha": float(alpha),
            "rerank": False,
        }
    if mode not in fixed or alpha is not None:
        raise AdapterError("invalid Semble execution profile")
    profile_id, fixed_alpha, rerank = fixed[mode]
    return {"profile_id": profile_id, "mode": mode, "alpha": fixed_alpha, "rerank": rerank}


WORKER_TEMPLATE = '''"""Spawned Semble worker (pinned env only). Reads SPEC_JSON, writes NATIVE_JSON."""
import json
import hashlib
import inspect
import os
import sys
import time
import contextlib

protocol_output = sys.stdout

def peak_resident_bytes() -> int:
    if sys.platform == "win32":
        import ctypes

        class ProcessMemoryCounters(ctypes.Structure):
            _fields_ = [
                ("cb", ctypes.c_uint32),
                ("PageFaultCount", ctypes.c_uint32),
                ("PeakWorkingSetSize", ctypes.c_size_t),
                ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                ("PagefileUsage", ctypes.c_size_t),
                ("PeakPagefileUsage", ctypes.c_size_t),
            ]

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        psapi.GetProcessMemoryInfo.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ProcessMemoryCounters),
            ctypes.c_uint32,
        ]
        psapi.GetProcessMemoryInfo.restype = ctypes.c_int32
        counters = ProcessMemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(
            kernel.GetCurrentProcess(), ctypes.byref(counters), counters.cb
        ):
            raise OSError(ctypes.get_last_error(), "GetProcessMemoryInfo failed")
        observed = int(counters.PeakWorkingSetSize)
        if observed <= 0:
            raise RuntimeError("Windows peak working set is not positive")
        return observed

    import resource

    unit = 1 if sys.platform == "darwin" else 1024
    observed = int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss) * unit
    if observed <= 0:
        raise RuntimeError("peak resident set is not positive")
    return observed

def main() -> int:
    worker_started_ns = time.monotonic_ns()
    spec_path = os.environ["SPEC_JSON"]
    out_path = os.environ["NATIVE_JSON"]
    with open(spec_path, encoding="utf-8") as handle:
        spec = json.load(handle)
    discovery_end_ns = time.monotonic_ns()
    from semble import SembleIndex
    import semble.search as semble_search
    from semble.types import ContentType
    model_prepare_end_ns = time.monotonic_ns()

    profile = str(spec.get("semble_profile", "native-default"))
    if profile not in (
        "native-default",
        "hybrid-no-rerank",
        "lexical-only",
        "lexical-file",
        "semantic-only",
    ):
        raise SystemExit(f"worker refuses unknown semble profile: {profile}")
    alpha = spec.get("alpha") if profile == "hybrid-no-rerank" else None
    if profile == "hybrid-no-rerank" and (
        type(alpha) not in (int, float) or not 0.0 <= alpha <= 1.0
    ):
        raise SystemExit("hybrid-no-rerank profile requires an explicit alpha in [0, 1]")
    alpha = float(alpha) if alpha is not None else None

    rss_before_index = peak_resident_bytes()
    index = SembleIndex.from_path(spec["corpus_dir"], show_progress_bar=False)
    index_end_ns = time.monotonic_ns()
    rss_after_index = peak_resident_bytes()
    index_resident_bytes = max(rss_after_index - rss_before_index, 0)
    if index_resident_bytes == 0:
        raise SystemExit("worker could not attribute positive resident bytes to the index")

    # Transparent wrappers are installed at the exact globals used by pinned
    # Semble 0.6.0: SembleIndex.search -> index.index.search ->
    # search.resolve_alpha/_search_* . Each wrapper calls its original once
    # and preserves the return value.
    events = []
    active = None
    observed_wrapped_call_ns = 0
    semble_index_module = sys.modules[index.__class__.__module__]
    _real_module_search = getattr(semble_index_module, "search", None)
    if _real_module_search is None or not callable(_real_module_search):
        raise SystemExit("pinned Semble index module lacks the module-local search boundary")
    _real_resolve_alpha = semble_search.resolve_alpha
    lane_calls = {"bm25": 0, "semantic": 0, "encode": 0}
    _real_search_bm25 = semble_search._search_bm25
    _real_search_semantic = semble_search._search_semantic

    def _source_identity(fn):
        raw = inspect.getsource(fn).encode("utf-8")
        return {
            "module": fn.__module__,
            "qualname": fn.__qualname__,
            "source_sha256": hashlib.sha256(raw).hexdigest(),
        }

    function_identity = {
        "index_search": _source_identity(index.__class__.search),
        "module_search": _source_identity(_real_module_search),
        "resolve_alpha": _source_identity(_real_resolve_alpha),
        "bm25": _source_identity(_real_search_bm25),
        "semantic": _source_identity(_real_search_semantic),
    }

    def _observed_resolve_alpha(query, requested):
        nonlocal observed_wrapped_call_ns
        started = time.monotonic_ns()
        resolved = _real_resolve_alpha(query, requested)
        observed_wrapped_call_ns += time.monotonic_ns() - started
        if active is None:
            raise SystemExit("resolve_alpha executed outside an observed dispatch")
        active["actual_alpha"] = float(resolved)
        return resolved

    def _observed_module_search(*args, **kwargs):
        nonlocal observed_wrapped_call_ns
        if active is None:
            raise SystemExit("Semble module search executed outside an observed dispatch")
        top_k_arg = args[5] if len(args) > 5 else kwargs.get("top_k")
        rerank_arg = args[8] if len(args) > 8 else kwargs.get("rerank", True)
        if type(top_k_arg) is not int or top_k_arg <= 0 or type(rerank_arg) is not bool:
            raise SystemExit("Semble module search arguments are not observable")
        active["actual_rerank"] = rerank_arg
        active["candidate_depth"] = top_k_arg * 5
        started = time.monotonic_ns()
        result = _real_module_search(*args, **kwargs)
        observed_wrapped_call_ns += time.monotonic_ns() - started
        return result

    def _counted_bm25(*args, **kwargs):
        lane_calls["bm25"] += 1
        if active is not None:
            active["lane_entry_counts"]["bm25"] += 1
            active["lane_candidate_depths"]["bm25"].append(
                int(args[3] if len(args) > 3 else kwargs["top_k"])
            )
        return _real_search_bm25(*args, **kwargs)

    def _counted_semantic(*args, **kwargs):
        lane_calls["semantic"] += 1
        if active is not None:
            active["lane_entry_counts"]["semantic"] += 1
            active["lane_candidate_depths"]["semantic"].append(
                int(args[4] if len(args) > 4 else kwargs["top_k"])
            )
        return _real_search_semantic(*args, **kwargs)

    semble_search.resolve_alpha = _observed_resolve_alpha
    semble_index_module.search = _observed_module_search
    semble_search._search_bm25 = _counted_bm25
    semble_search._search_semantic = _counted_semantic
    _real_encode = getattr(index.model, "encode", None)
    if _real_encode is not None:
        def _counted_encode(*args, **kwargs):
            lane_calls["encode"] += 1
            return _real_encode(*args, **kwargs)

        index.model.encode = _counted_encode

    if profile in ("lexical-only", "lexical-file", "semantic-only") and not all(
        hasattr(semble_search, name) for name in ("_search_bm25", "_search_semantic")
    ):
        raise SystemExit(
            f"pinned Semble build does not expose single-lane functions; {profile} is unsupported"
        )

    def dispatch(query, top_k, *, rep, phase, phase_iteration, task_id):
        nonlocal active
        event = {
            "rep": rep,
            "phase": phase,
            "phase_iteration": phase_iteration,
            "task_id": task_id,
            "call_ordinal": len(events),
            "submitted_query_sha256": hashlib.sha256(query.encode("utf-8")).hexdigest(),
            "profile_sha256": spec["execution_profile_sha256"],
            "actual_alpha": None,
            "actual_rerank": None,
            "candidate_depth": (
                len(index.chunks) if profile == "lexical-file"
                else top_k if profile in ("lexical-only", "semantic-only") else None
            ),
            "lane_entry_counts": {"bm25": 0, "semantic": 0},
            "lane_candidate_depths": {"bm25": [], "semantic": []},
        }
        active = event
        protocol_output.write(json.dumps({
            "kind": "request_ready", "task_id": task_id, "phase": phase,
            "iteration": phase_iteration, "indexed_chunks": len(index.chunks),
        }) + "\\n")
        protocol_output.flush()
        request = json.loads(sys.stdin.readline())
        if request != {"task_id": task_id, "query": query, "top_k": top_k}:
            raise SystemExit("parent request differs from the frozen worker schedule")
        # Single dispatch shared by cold, warmup, and measured phases
        # (RBR-03): no phase can run a different profile.
        try:
            if profile == "native-default":
                result = index.search(query, top_k=top_k)
            elif profile == "hybrid-no-rerank":
                result = index.search(query, top_k=top_k, alpha=alpha, rerank=False)
            elif profile == "lexical-only":
                result = semble_search._search_bm25(
                    query, index._bm25_index, index.chunks, top_k, None
                )
            elif profile == "lexical-file":
                # Preserve the full positive-score native order in the raw
                # capture. The adapter proves every row before selecting the
                # first ten distinct file paths.
                result = semble_search._search_bm25(
                    query, index._bm25_index, index.chunks, len(index.chunks), None
                )
            else:
                result = semble_search._search_semantic(
                    query, index.model, index._semantic_index, index.chunks, top_k, None
                )
        finally:
            events.append(event)
            active = None
        native_result = [
            {"file_path": hit.chunk.file_path, "start_line": int(hit.chunk.start_line),
             "end_line": int(hit.chunk.end_line), "score": float(hit.score)}
            for hit in result
        ]
        # Encode once for both the completed-response pipe and the immutable
        # native capture. The pipe adds only its protocol kind field.
        native_row = json.dumps(
            {"task_id": task_id, "results": native_result},
            sort_keys=True,
            separators=(",", ":"),
        )
        protocol_output.write('{"kind":"response",' + native_row[1:] + "\\n")
        protocol_output.flush()
        observation = json.loads(sys.stdin.readline())
        if observation.get("task_id") != task_id or observation.get("phase") != phase:
            raise SystemExit("parent completed-response observation differs from the request")
        completed_calls.append(observation)
        return native_row

    observed = sorted({chunk.file_path for chunk in index.chunks})
    stats = {
        "indexed_files": int(index.stats.indexed_files),
        "total_chunks": int(index.stats.total_chunks),
        "languages": {str(k): int(v) for k, v in dict(index.stats.languages).items()},
        "index_resident_bytes": index_resident_bytes,
        "index_measurement": "process_peak_rss_delta_v1",
    }
    queries = [(task["task_id"], task["query"]) for task in spec["tasks"]]
    if len({task_id for task_id, _ in queries}) != len(queries):
        raise SystemExit("worker refuses a spec with duplicate task_ids")
    top_k = int(spec["top_k"])
    seed = int(spec.get("seed", 0))
    warmup = int(spec.get("warmup_passes", 1))
    repetitions = int(spec.get("repetitions", 1))
    protocol = spec.get("query_protocol")
    query_by_id = dict(queries)
    completed_calls = []
    cold_latency_ms = None
    cold_query_start_ns = index_end_ns
    cold_query_end_ns = index_end_ns
    if protocol is not None:
        cold_query_start_ns = time.monotonic_ns()
        task_id = protocol["cold_probe_task_id"]
        dispatch(query_by_id[task_id], top_k, rep=0, phase="cold", phase_iteration=0, task_id=task_id)
        cold_query_end_ns = time.monotonic_ns()
        cold_latency_ms = (completed_calls[-1]["end_ns"] - completed_calls[-1]["start_ns"]) / 1_000_000.0
        warmup_schedules = protocol["warmup_schedules"]
        measurement_schedules = protocol["measurement_schedules"]
    else:
        warmup_schedules = [[task_id for task_id, _ in queries] for _ in range(warmup)]
        measurement_schedules = [[task_id for task_id, _ in queries] for _ in range(repetitions)]
    for warmup_iteration, schedule in enumerate(warmup_schedules):
        for task_id in schedule:
            dispatch(query_by_id[task_id], top_k, rep=0, phase="warmup", phase_iteration=warmup_iteration, task_id=task_id)
    warmup_end_ns = time.monotonic_ns()
    # Retain encoded rows instead of millions of per-hit Python objects until
    # the final native artifact is written. The full ordered hits remain in it.
    native_rows = []
    native_task_ids = []
    latencies = {}
    actual_alpha_by_task = {}
    query_started_ns = time.monotonic_ns()
    first_query_ms = None
    first_query_start_ns = None
    first_query_end_ns = None
    for rep, schedule in enumerate(measurement_schedules):
        for task_id in schedule:
            query = query_by_id[task_id]
            t0 = time.monotonic_ns()
            native_row = dispatch(query, top_k, rep=rep, phase="measured", phase_iteration=rep, task_id=task_id)
            ended_ns = time.monotonic_ns()
            elapsed_ms = (completed_calls[-1]["end_ns"] - completed_calls[-1]["start_ns"]) / 1_000_000.0
            if first_query_ms is None:
                first_query_ms = elapsed_ms
                first_query_start_ns = t0
                first_query_end_ns = ended_ns
            latencies.setdefault(task_id, []).append(elapsed_ms)
            if rep == 0:
                native_rows.append(native_row)
                native_task_ids.append(task_id)
                if profile in ("native-default", "hybrid-no-rerank"):
                    actual_alpha_by_task[task_id] = events[-1]["actual_alpha"]
    query_end_ns = time.monotonic_ns()
    expected_dispatch_count = sum(map(len, warmup_schedules)) + sum(
        map(len, measurement_schedules)
    ) + (1 if protocol is not None else 0)
    if profile in ("native-default", "hybrid-no-rerank"):
        expected_rerank = profile == "native-default"
        if [event["actual_rerank"] for event in events] != [expected_rerank] * expected_dispatch_count:
            raise SystemExit(
                "Semble rerank trace differs from the requested profile: "
                f"observed={[event['actual_rerank'] for event in events]} expected={expected_rerank} "
                f"calls={expected_dispatch_count}"
            )
        rerank_applied = expected_rerank
    else:
        if any(event["actual_rerank"] is not None for event in events):
            raise SystemExit("pure-lane profile unexpectedly used hybrid search")
        rerank_applied = False
    # RBR-03 fail-closed lane invariants, checked at the source before the
    # payload leaves the worker.
    if profile in ("lexical-only", "lexical-file") and (lane_calls["semantic"] or lane_calls["encode"]):
        raise SystemExit(
            f"lexical-only profile executed semantic/encode lanes: {lane_calls}"
        )
    if profile in ("lexical-only", "lexical-file") and not lane_calls["bm25"]:
        raise SystemExit(f"lexical-only profile ran no BM25 lane: {lane_calls}")
    if profile == "semantic-only" and lane_calls["bm25"]:
        raise SystemExit(f"semantic-only profile executed the BM25 lane: {lane_calls}")
    if profile == "semantic-only" and not lane_calls["semantic"]:
        raise SystemExit(f"semantic-only profile ran no semantic lane: {lane_calls}")
    if profile in ("native-default", "hybrid-no-rerank") and not (
        lane_calls["bm25"] and lane_calls["semantic"]
    ):
        raise SystemExit(
            f"{profile} profile must run both lanes (alpha endpoints are score "
            f"ablations, not single-lane runs): {lane_calls}"
        )
    if lane_calls["encode"] != lane_calls["semantic"]:
        raise SystemExit(
            "Semble semantic lane and model encode call counts differ: "
            f"{lane_calls}"
        )
    first_query_ms = first_query_ms or 0.0
    if first_query_start_ns is None or first_query_end_ns is None:
        raise SystemExit("worker did not execute a first measured query")
    emitted = sorted(native_task_ids)
    expected = sorted(task_id for task_id, _ in queries)
    if emitted != expected:
        raise SystemExit("worker output task set differs from the spec task set")
    worker_end_ns = time.monotonic_ns()
    query_ms = (query_end_ns - query_started_ns) / 1_000_000.0
    phase_boundaries_ns = {
        "worker_start": worker_started_ns,
        "discovery_end": discovery_end_ns,
        "model_provider_prepare_end": model_prepare_end_ns,
        "index_end": index_end_ns,
        "warmup_end": warmup_end_ns,
        "query_start": query_started_ns,
        "first_query_start": first_query_start_ns,
        "first_query_end": first_query_end_ns,
        "cold_query_start": cold_query_start_ns,
        "cold_query_end": cold_query_end_ns,
        "query_end": query_end_ns,
        "worker_end": worker_end_ns,
    }
    payload = {
        "semble_profile": profile,
        "requested_alpha": alpha if profile == "hybrid-no-rerank" else None,
        "actual_alpha_by_task": (
            actual_alpha_by_task
            if profile in ("native-default", "hybrid-no-rerank")
            else None
        ),
        "execution_events": events,
        "execution_events_sha256": hashlib.sha256(
            json.dumps(events, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest(),
        "function_identity": function_identity,
        # This includes the wrapped upstream calls. It is evidence about the
        # observation boundary, never a value to subtract from query latency.
        "observed_wrapped_call_ns": observed_wrapped_call_ns,
        "rerank_applied": rerank_applied,
        "lane_call_counts": dict(lane_calls),
        "semble_index_ms": (index_end_ns - model_prepare_end_ns) / 1_000_000.0,
        "discovery_ms": (discovery_end_ns - worker_started_ns) / 1_000_000.0,
        "model_provider_prepare_ms": (
            model_prepare_end_ns - discovery_end_ns
        ) / 1_000_000.0,
        "warmup_ms": (
            warmup_end_ns - (cold_query_end_ns if protocol is not None else index_end_ns)
        ) / 1_000_000.0,
        "first_query_ms": (first_query_end_ns - first_query_start_ns) / 1_000_000.0,
        "warm_query_ms": max(query_ms - (first_query_end_ns - first_query_start_ns) / 1_000_000.0, 0.0),
        "cold_query_ms": (cold_query_end_ns - cold_query_start_ns) / 1_000_000.0,
        "protocol_warm_query_ms": query_ms if protocol is not None else None,
        "worker_total_ms": (worker_end_ns - worker_started_ns) / 1_000_000.0,
        "phase_boundaries_ns": phase_boundaries_ns,
        "configured_model_name": os.environ["SEMBLE_MODEL_NAME"],
        "observed_files": observed,
        "stats": stats,
        "query_schedule": [task_id for task_id, _ in queries],
        "query_protocol": protocol,
        "cold_latency_ms": cold_latency_ms,
        "native": [],
        "latencies_ms": latencies,
        "query_timing": {
            "boundary": "request_construction_to_normalized_response",
            "clock": "capture_relative_monotonic_ns",
            "output_validation": "@completed_output_validation@",
            "observations": completed_calls,
        },
        "worker_pid": os.getpid(),
        "repetitions": repetitions,
        "warmup_passes": warmup,
        "seed": seed,
    }
    rendered = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    marker = '"native":[]'
    if rendered.count(marker) != 1:
        raise SystemExit("native artifact lacks one result slot")
    before, after = rendered.split(marker)
    with open(out_path, "x", encoding="utf-8") as handle:
        handle.write(before)
        handle.write('"native":[')
        for ordinal, row in enumerate(native_rows):
            if ordinal:
                handle.write(",")
            handle.write(row)
        handle.write("]")
        handle.write(after)
        handle.write("\\n")
    protocol_output.write(json.dumps({"kind": "finished"}) + "\\n")
    protocol_output.flush()
    return 0


if __name__ == "__main__":
    with contextlib.redirect_stdout(sys.stderr):
        raise SystemExit(main())
'''.replace("@completed_output_validation@", COMPLETED_OUTPUT_VALIDATION)


SEMBLE_PROFILES = (
    "native-default",
    "hybrid-no-rerank",
    "lexical-only",
    "lexical-file",
    "semantic-only",
)


def validate_native_profile_report(
    native_payload: dict,
    profile: str,
    alpha: float | None,
    *,
    expected_query_sha256: dict[str, str] | None = None,
) -> None:
    """Reject a worker report that disagrees with the requested profile.

    RBR-03: the worker must echo the requested profile and prove lane
    isolation; a mismatched or forged execution report refuses the capture.
    """
    if native_payload.get("semble_profile") != profile:
        raise AdapterError(
            "Semble worker executed a different profile than requested: "
            f"{native_payload.get('semble_profile')!r} != {profile!r}"
        )
    lane_counts = native_payload.get("lane_call_counts")
    if not isinstance(lane_counts, dict) or set(lane_counts) != {
        "bm25",
        "semantic",
        "encode",
    }:
        raise AdapterError("Semble worker lane-call report is malformed")
    if any(type(count) is not int or count < 0 for count in lane_counts.values()):
        raise AdapterError("Semble worker lane-call report is invalid")
    if profile in ("lexical-only", "lexical-file") and (
        lane_counts["semantic"] or lane_counts["encode"]
    ):
        raise AdapterError("lexical-only capture executed semantic/encode lanes")
    if profile in ("lexical-only", "lexical-file") and not lane_counts["bm25"]:
        raise AdapterError("lexical-only capture ran no BM25 lane")
    if profile == "semantic-only" and lane_counts["bm25"]:
        raise AdapterError("semantic-only capture executed the BM25 lane")
    if profile == "semantic-only" and not lane_counts["semantic"]:
        raise AdapterError("semantic-only capture ran no semantic lane")
    if profile in ("native-default", "hybrid-no-rerank") and not (
        lane_counts["bm25"] and lane_counts["semantic"]
    ):
        raise AdapterError(
            f"{profile} capture must run both lanes; alpha endpoints are score ablations"
        )
    if profile == "native-default" and native_payload.get("rerank_applied") is not True:
        raise AdapterError("native-default capture must prove rerank_applied=True")
    if profile != "native-default" and native_payload.get("rerank_applied") is not False:
        if profile == "hybrid-no-rerank":
            raise AdapterError("hybrid-no-rerank capture must report rerank disabled")
        raise AdapterError(f"{profile} capture must prove rerank_applied=False")
    if profile == "hybrid-no-rerank":
        if native_payload.get("requested_alpha") != alpha:
            raise AdapterError("Semble worker alpha echo differs from the requested alpha")
        if native_payload.get("rerank_applied") is not False:
            raise AdapterError("hybrid-no-rerank capture must report rerank disabled")
    if profile != "hybrid-no-rerank" and native_payload.get("requested_alpha") is not None:
        raise AdapterError(f"{profile} capture must not report a requested alpha")
    events = native_payload.get("execution_events")
    if not isinstance(events, list) or not events:
        raise AdapterError("Semble worker execution event report is absent")
    identities = native_payload.get("function_identity")
    if not isinstance(identities, dict) or set(identities) != {
        "bm25",
        "index_search",
        "module_search",
        "resolve_alpha",
        "semantic",
    }:
        raise AdapterError("Semble function identity report is malformed")
    for name, identity in identities.items():
        if (
            not isinstance(identity, dict)
            or set(identity) != {"module", "qualname", "source_sha256"}
            or not isinstance(identity["module"], str)
            or not identity["module"]
            or not isinstance(identity["qualname"], str)
            or not identity["qualname"]
            or not isinstance(identity["source_sha256"], str)
            or len(identity["source_sha256"]) != 64
            or any(ch not in "0123456789abcdef" for ch in identity["source_sha256"])
        ):
            raise AdapterError(f"Semble function identity is invalid: {name}")
    observed_ns = native_payload.get("observed_wrapped_call_ns")
    if type(observed_ns) is not int or observed_ns < 0:
        raise AdapterError("Semble wrapped-call duration is invalid")

    native_rows = native_payload.get("native")
    if not isinstance(native_rows, list) or not native_rows:
        raise AdapterError("Semble native result rows are absent")
    native_task_order = []
    for row in native_rows:
        if not isinstance(row, dict) or not isinstance(row.get("task_id"), str):
            raise AdapterError("Semble native result row is malformed")
        native_task_order.append(row["task_id"])
    if len(native_task_order) != len(set(native_task_order)):
        raise AdapterError("Semble native task rows are duplicated")
    query_schedule = native_payload.get("query_schedule")
    if (
        not isinstance(query_schedule, list)
        or any(not isinstance(task_id, str) or not task_id for task_id in query_schedule)
        or len(query_schedule) != len(set(query_schedule))
        or set(query_schedule) != set(native_task_order)
    ):
        raise AdapterError("Semble query schedule differs from native task rows")
    repetitions = native_payload.get("repetitions")
    warmup_passes = native_payload.get("warmup_passes")
    if type(repetitions) is not int or repetitions <= 0:
        raise AdapterError("Semble repetitions are invalid")
    if type(warmup_passes) is not int or warmup_passes < 0:
        raise AdapterError("Semble warmup passes are invalid")
    protocol = native_payload.get("query_protocol")
    expected_events: list[tuple[int, str, int, str]] = []
    if protocol is not None:
        protocol = validate_query_protocol(protocol, query_schedule)
        if (
            len(protocol["warmup_schedules"]) != warmup_passes
            or len(protocol["measurement_schedules"]) != repetitions
        ):
            raise AdapterError("Semble query protocol counts differ from worker settings")
        expected_events.append((0, "cold", 0, protocol["cold_probe_task_id"]))
        warmup_schedules = protocol["warmup_schedules"]
        measurement_schedules = protocol["measurement_schedules"]
    else:
        if len(events) != len(query_schedule) * (repetitions + warmup_passes):
            raise AdapterError("Semble execution event count differs from the query protocol")
        warmup_schedules = [query_schedule for _ in range(warmup_passes)]
        measurement_schedules = [query_schedule for _ in range(repetitions)]
    for iteration, schedule in enumerate(warmup_schedules):
        expected_events.extend((0, "warmup", iteration, task_id) for task_id in schedule)
    for repetition, schedule in enumerate(measurement_schedules):
        expected_events.extend(
            (repetition, "measured", repetition, task_id) for task_id in schedule
        )
    if native_task_order != measurement_schedules[0]:
        raise AdapterError("Semble native row order differs from the first measured schedule")
    if expected_query_sha256 is not None and set(expected_query_sha256) != set(query_schedule):
        raise AdapterError("Semble expected query identity set differs from the query schedule")
    if len(events) != len(expected_events):
        raise AdapterError("Semble execution event count differs from the query protocol")
    stats = native_payload.get("stats")
    indexed_chunks = stats.get("total_chunks") if isinstance(stats, dict) else None
    if profile == "lexical-file" and (type(indexed_chunks) is not int or indexed_chunks <= 0):
        raise AdapterError("Semble file collection lacks indexed chunk count")
    expected_profile_sha = digest(canonical(execution_profile(profile, alpha)))
    measured: dict[tuple[int, str], dict] = {}
    observed_lane_calls = {"bm25": 0, "semantic": 0}
    for ordinal, (event, expected_event) in enumerate(zip(events, expected_events, strict=True)):
        required = {
            "rep",
            "phase",
            "phase_iteration",
            "task_id",
            "call_ordinal",
            "submitted_query_sha256",
            "profile_sha256",
            "actual_alpha",
            "actual_rerank",
            "candidate_depth",
            "lane_entry_counts",
            "lane_candidate_depths",
        }
        if not isinstance(event, dict) or set(event) != required:
            raise AdapterError("Semble worker execution event shape is invalid")
        if any(
            type(event[name]) is not int or event[name] < 0
            for name in ("rep", "phase_iteration", "call_ordinal")
        ):
            raise AdapterError("Semble execution event integer identity is invalid")
        key = tuple(event[name] for name in ("rep", "phase", "phase_iteration", "task_id"))
        if key != expected_event or event["call_ordinal"] != ordinal:
            raise AdapterError("Semble execution event order differs from the query protocol")
        if event["profile_sha256"] != expected_profile_sha:
            raise AdapterError("Semble worker execution event profile digest mismatch")
        query_sha = event["submitted_query_sha256"]
        if (
            not isinstance(query_sha, str)
            or len(query_sha) != 64
            or any(ch not in "0123456789abcdef" for ch in query_sha)
            or expected_query_sha256 is not None
            and query_sha != expected_query_sha256[event["task_id"]]
        ):
            raise AdapterError("Semble execution event query identity mismatch")
        lane_event = event["lane_entry_counts"]
        depths = event["lane_candidate_depths"]
        if (
            not isinstance(lane_event, dict)
            or set(lane_event) != {"bm25", "semantic"}
            or not isinstance(depths, dict)
            or set(depths) != {"bm25", "semantic"}
            or any(type(value) is not int or value < 0 for value in lane_event.values())
            or any(
                not isinstance(value, list)
                or any(type(depth) is not int or depth <= 0 for depth in value)
                for value in depths.values()
            )
        ):
            raise AdapterError("Semble worker per-event lane report is invalid")
        candidate_depth = event["candidate_depth"]
        if type(candidate_depth) is not int or candidate_depth <= 0:
            raise AdapterError("Semble event candidate depth is invalid")
        if profile == "lexical-file" and candidate_depth != indexed_chunks:
            raise AdapterError("Semble file collection did not request every indexed chunk")
        for lane in observed_lane_calls:
            if len(depths[lane]) != lane_event[lane] or any(
                depth != candidate_depth for depth in depths[lane]
            ):
                raise AdapterError("Semble event candidate depth differs from lane calls")
            observed_lane_calls[lane] += lane_event[lane]
        if profile in ("native-default", "hybrid-no-rerank"):
            if lane_event != {"bm25": 1, "semantic": 1}:
                raise AdapterError("Semble hybrid event did not enter both lanes exactly once")
            if event["actual_rerank"] is not (profile == "native-default"):
                raise AdapterError("Semble event rerank differs from the profile")
            if (
                not is_finite_json_number(event["actual_alpha"])
                or not 0 <= event["actual_alpha"] <= 1
            ):
                raise AdapterError("Semble event actual alpha is invalid")
            if profile == "hybrid-no-rerank" and event["actual_alpha"] != alpha:
                raise AdapterError("Semble event actual alpha differs from requested alpha")
        elif profile in ("lexical-only", "lexical-file"):
            if lane_event != {"bm25": 1, "semantic": 0} or depths["semantic"]:
                raise AdapterError("Semble lexical event crossed the lane boundary")
        elif lane_event != {"bm25": 0, "semantic": 1} or depths["bm25"]:
            raise AdapterError("Semble semantic event crossed the lane boundary")
        if event["phase"] == "measured":
            measured[(event["rep"], event["task_id"])] = event
    if native_payload.get("execution_events_sha256") != digest(canonical(events)):
        raise AdapterError("Semble execution event digest mismatch")
    if any(lane_counts[lane] != count for lane, count in observed_lane_calls.items()):
        raise AdapterError("Semble aggregate lane calls differ from execution events")
    if lane_counts["encode"] != lane_counts["semantic"]:
        raise AdapterError("Semble semantic and encode call counts differ")
    if set(measured) != {
        (rep, task_id) for rep in range(repetitions) for task_id in query_schedule
    }:
        raise AdapterError("Semble measured execution event coverage is incomplete")
    expected_alpha_by_task = (
        {task_id: measured[(0, task_id)]["actual_alpha"] for task_id in measurement_schedules[0]}
        if profile in ("native-default", "hybrid-no-rerank")
        else None
    )
    if native_payload.get("actual_alpha_by_task") != expected_alpha_by_task:
        raise AdapterError("Semble actual alpha summary differs from measured events")


class AdapterError(ValueError):
    """Semble adapter evidence is absent, inconsistent or ineligible."""


def _int(value: object, label: str) -> int:
    try:
        return int(str(value))
    except (TypeError, ValueError) as exc:
        raise AdapterError(f"{label} must be an integer: {value!r}") from exc


def read_json(path: Path) -> object:
    def unique_object(pairs: list[tuple[str, object]]) -> dict:
        value = {}
        for key, item in pairs:
            if key in value:
                raise AdapterError(f"duplicate JSON key: {key}")
            value[key] = item
        return value

    def reject_constant(value: str) -> object:
        raise AdapterError(f"non-finite JSON number: {value}")

    try:
        return json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=unique_object,
            parse_constant=reject_constant,
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise AdapterError(f"cannot read JSON {path}: {exc}") from exc


def sha_file(path: Path) -> str:
    digestor = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(65536), b""):
            digestor.update(block)
    return digestor.hexdigest()


def validate_query_protocol(payload: object, task_ids: list[str]) -> dict:
    if not isinstance(payload, dict):
        raise AdapterError("query protocol must be an object")
    expected_keys = {
        "schema_version",
        "seed",
        "task_ids",
        "cold_probe_task_id",
        "warmup_schedules",
        "measurement_schedules",
        "sha256",
    }
    if set(payload) != expected_keys:
        raise AdapterError("query protocol keys differ from the closed schema")
    if payload["schema_version"] != 1 or type(payload["seed"]) is not int:
        raise AdapterError("query protocol version or seed is invalid")
    if payload["task_ids"] != task_ids or payload["cold_probe_task_id"] not in task_ids:
        raise AdapterError("query protocol task ids differ from the query pack")
    expected = set(task_ids)
    for key in ("warmup_schedules", "measurement_schedules"):
        schedules = payload[key]
        if not isinstance(schedules, list) or (key == "measurement_schedules" and not schedules):
            raise AdapterError(f"query protocol {key} has an invalid schedule list")
        if any(
            not isinstance(schedule, list)
            or len(schedule) != len(task_ids)
            or any(not isinstance(task_id, str) for task_id in schedule)
            or set(schedule) != expected
            for schedule in schedules
        ):
            raise AdapterError(f"query protocol {key} must contain exact task permutations")
    core = {key: value for key, value in payload.items() if key != "sha256"}
    observed = hashlib.sha256(
        json.dumps(core, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    ).hexdigest()
    if payload["sha256"] != observed:
        raise AdapterError("query protocol digest mismatch")
    return payload


def load_manifest(path: Path) -> tuple[str, list[tuple[str, str]]]:
    payload = read_json(path)
    if not isinstance(payload, dict):
        raise AdapterError("manifest must be an object")
    commit = payload.get("repository_commit")
    files = payload.get("files")
    if (
        not isinstance(commit, str)
        or len(commit) != 40
        or any(c not in "0123456789abcdef" for c in commit)
    ):
        raise AdapterError("manifest lacks repository_commit")
    if not isinstance(files, list) or not files:
        raise AdapterError("manifest admits no files")
    rows = []
    for entry in files:
        if not isinstance(entry, dict):
            raise AdapterError("manifest entry must be an object")
        name, sha = entry.get("path"), entry.get("file_sha256")
        if not isinstance(name, str) or not name or not isinstance(sha, str):
            raise AdapterError("manifest entry lacks path/file_sha256")
        if (
            name.startswith("/")
            or "\\" in name
            or any(part in ("", ".", "..") for part in name.split("/"))
        ):
            raise AdapterError(f"unsafe manifest path: {name}")
        if len(sha) != 64 or any(c not in "0123456789abcdef" for c in sha):
            raise AdapterError(f"manifest entry has invalid file_sha256: {name}")
        rows.append((name, sha))
    if len({name for name, _ in rows}) != len(rows):
        raise AdapterError("manifest holds duplicate paths")
    return commit, rows


def load_query_pack(path: Path) -> dict:
    payload = read_json(path)
    if not isinstance(payload, dict):
        raise AdapterError("query pack must be an object")
    if payload.get("schema_version") != 3:
        raise AdapterError("query pack schema_version must be 3")
    expected = {
        "schema_version",
        "suite_id",
        "suite_commitment_sha256",
        "repository_commit",
        "tokenizer",
        "tokenizer_budget_version",
        "routes",
        "file_universe",
        "file_universe_digest",
        "comparison_contract",
        "tasks",
    }
    if set(payload) != expected:
        raise AdapterError("query pack holds unexpected or missing top-level keys")
    try:
        validate_comparison_contract(payload.get("comparison_contract"), "pack.comparison_contract")
    except ValueError as exc:
        raise AdapterError(f"query pack comparison contract invalid: {exc}") from exc
    tasks = payload.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise AdapterError("query pack holds no tasks")
    seen = set()
    for task in tasks:
        if not isinstance(task, dict):
            raise AdapterError("query pack task must be an object")
        if set(task) != {"task_id", "query", "query_sha256"}:
            raise AdapterError(
                "query pack task holds unexpected keys (gold/grade smuggling refused)"
            )
        for key in ("task_id", "query", "query_sha256"):
            if not isinstance(task.get(key), str) or not task[key]:
                raise AdapterError(f"query pack task lacks {key}")
        if task["task_id"] in seen:
            raise AdapterError(f"query pack holds a duplicate task_id: {task['task_id']!r}")
        seen.add(task["task_id"])
    return payload


def verify_lockfile(lockfile_bytes: bytes, expected_sha256: str, freeze_text: str) -> str:
    """Check the externally digest-pinned freeze against the entire observed env.

    The authoritative pin is the external lockfile (path + digest), never the
    observed ``pip freeze`` output: freeze is the env observation, the lockfile
    is the expectation. Every installed distribution must match exactly; a
    subset check would admit undeclared dependencies that can affect ranking.
    The file digest pins this environment snapshot, not individual wheel bytes.
    """
    if not isinstance(expected_sha256, str) or len(expected_sha256) != 64:
        raise AdapterError("lockfile pin must be a lowercase sha256")
    observed = hashlib.sha256(bytes(lockfile_bytes)).hexdigest()
    if observed != expected_sha256:
        raise AdapterError("external lockfile digest differs from the spec pin")

    def lines(raw: str, label: str) -> set[str]:
        entries = [line.strip() for line in raw.splitlines() if line.strip()]
        entries = [line for line in entries if not line.startswith("#")]
        if len(entries) != len(set(entries)):
            raise AdapterError(f"{label} has duplicate distribution lines")
        names: set[str] = set()
        for line in entries:
            if line.count("==") != 1 or any(char.isspace() for char in line):
                raise AdapterError(f"{label} has a non-version-pinned distribution line")
            name, version = line.split("==")
            if not name or not version:
                raise AdapterError(f"{label} has a non-version-pinned distribution line")
            normalized_name = name.lower().replace("_", "-").replace(".", "-")
            if normalized_name in names:
                raise AdapterError(f"{label} pins a distribution more than once")
            names.add(normalized_name)
        return set(entries)

    try:
        lock_text = lockfile_bytes.decode("utf-8", "strict")
    except UnicodeDecodeError as exc:
        raise AdapterError("external lockfile is not UTF-8") from exc
    locked = lines(lock_text, "external lockfile")
    frozen = lines(freeze_text, "observed freeze")
    if f"semble=={SEMBLE_PINNED_VERSION}" not in locked:
        raise AdapterError("external lockfile lacks the pinned Semble line")
    if f"semble=={SEMBLE_PINNED_VERSION}" not in frozen:
        raise AdapterError("observed freeze lacks the pinned Semble line")
    missing = sorted(locked - frozen)
    if missing:
        raise AdapterError(f"observed freeze lacks {len(missing)} locked lines")
    extra = sorted(frozen - locked)
    if extra:
        raise AdapterError(f"observed freeze has {len(extra)} unlocked lines")
    return observed


def check_semble_env(python: Path) -> dict:
    """Verify the pinned Semble interpreter: exact version, imports, identity."""
    if not python.is_file():
        raise AdapterError(f"Semble python is not a file: {python}")
    probe = (
        "import hashlib, importlib.metadata, json, pathlib, sys; "
        "import semble; "
        "from semble import SembleIndex; "
        "pkg = pathlib.Path(semble.__file__).resolve().parent; "
        "infos = sorted(pkg.parent.glob('semble-*.dist-info')); "
        "info = infos[0] if infos else None; "
        "record = (info / 'RECORD').read_bytes() if info and (info / 'RECORD').is_file() else None; "
        "direct = (info / 'direct_url.json').read_bytes() if info and (info / 'direct_url.json').is_file() else None; "
        "print(json.dumps({"
        "'semble_version': importlib.metadata.version('semble'), "
        "'python_version': sys.version, "
        "'has_from_path': hasattr(SembleIndex, 'from_path'), "
        "'has_search': hasattr(SembleIndex, 'search'), "
        "'dist_info': info.name if info else None, "
        "'record_sha256': hashlib.sha256(record).hexdigest() if record else None, "
        "'direct_url_sha256': hashlib.sha256(direct).hexdigest() if direct else None}))"
    )
    try:
        completed = subprocess.run(
            [str(python), "-c", probe],
            check=True,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
        raise AdapterError(f"Semble env probe failed: {exc}") from exc
    try:
        report = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise AdapterError(f"Semble env probe is not JSON: {exc}") from exc
    if not report.get("has_from_path") or not report.get("has_search"):
        raise AdapterError("pinned Semble lacks the from_path/search API")
    observed_version = report.get("semble_version")
    if observed_version != SEMBLE_PINNED_VERSION:
        raise AdapterError(
            f"Semble {SEMBLE_PINNED_VERSION} is pinned but the env holds {observed_version!r}"
        )
    if not isinstance(report.get("python_version"), str) or not report["python_version"]:
        raise AdapterError("Semble env probe lacks the interpreter version")
    installed = {
        "dist_info": report.get("dist_info"),
        "record_sha256": report.get("record_sha256"),
        "direct_url_sha256": report.get("direct_url_sha256"),
    }
    if not isinstance(installed["dist_info"], str) or not installed["dist_info"]:
        raise AdapterError("installed Semble lacks dist-info identity proof")
    record_digest = installed["record_sha256"]
    if (
        not isinstance(record_digest, str)
        or len(record_digest) != 64
        or any(c not in "0123456789abcdef" for c in record_digest)
    ):
        raise AdapterError("installed Semble lacks a RECORD digest proof")
    direct_digest = installed["direct_url_sha256"]
    if direct_digest is not None and (
        not isinstance(direct_digest, str)
        or len(direct_digest) != 64
        or any(c not in "0123456789abcdef" for c in direct_digest)
    ):
        raise AdapterError("installed Semble holds a malformed direct_url digest")
    report["installed_distribution"] = installed
    try:
        freeze = subprocess.run(
            [str(python), "-m", "pip", "freeze"],
            check=False,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise AdapterError(f"Semble environment pip freeze failed: {exc}") from exc
    if freeze.returncode != 0 or not freeze.stdout.strip():
        raise AdapterError("Semble environment pip freeze failed or is empty")
    # Freeze is the env observation, never the authoritative pin: the external
    # digest-pinned environment snapshot is the expectation (see verify_lockfile).
    report["observed_freeze"] = freeze.stdout
    report["observed_freeze_sha256"] = hashlib.sha256(freeze.stdout.encode("utf-8")).hexdigest()
    resolved = python.resolve()
    try:
        interpreter_digest = sha_file(resolved)
    except OSError as exc:
        raise AdapterError(f"cannot hash the Semble interpreter: {exc}") from exc
    report["interpreter"] = {
        "path": str(python),
        "realpath": str(resolved),
        "version": report.pop("python_version"),
        "digest": interpreter_digest,
    }
    return report


def build_isolated_corpus(
    repo: Path, manifest_rows: list[tuple[str, str]], corpus_dir: Path
) -> tuple[list[tuple[str, str]], int]:
    """Copy admitted bytes to an isolated dir. Returns rows + max bytes."""
    if corpus_dir.exists():
        raise AdapterError(f"corpus dir already exists (refusing reuse): {corpus_dir}")
    corpus_dir.mkdir(parents=True)
    rows: list[tuple[str, str]] = []
    max_bytes = 0
    for name, expected in manifest_rows:
        source = repo / name
        if source.is_symlink() or not source.is_file() or repo not in source.resolve().parents:
            raise AdapterError(f"admitted file is not a regular file: {name}")
        data = source.read_bytes()
        observed = hashlib.sha256(data).hexdigest()
        if observed != expected:
            raise AdapterError(f"admitted file hash mismatch: {name}")
        target = corpus_dir / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        rows.append((name, observed))
        max_bytes = max(max_bytes, len(data))
    return rows, max_bytes


def verify_materialized_corpus(repo: Path, manifest_rows: list[tuple[str, str]]) -> Path:
    """Prove a Git-free directory contains exactly the admitted file universe."""
    repo = repo.resolve()
    if not repo.is_dir() or (repo / ".git").exists():
        raise AdapterError("materialized corpus must be a Git-free directory")
    observed: list[tuple[str, str]] = []
    for dirpath, dirnames, filenames in os.walk(repo, followlinks=False):
        base = Path(dirpath)
        if any((base / name).is_symlink() for name in dirnames):
            raise AdapterError("materialized corpus contains a symlinked directory")
        for name in filenames:
            path = base / name
            if path.is_symlink() or not path.is_file():
                raise AdapterError("materialized corpus contains a non-regular file")
            observed.append((path.relative_to(repo).as_posix(), sha_file(path)))
    if sorted(observed) != sorted(manifest_rows):
        raise AdapterError("materialized corpus differs from the admitted universe")
    return repo


def read_hf_revision(hf_home: Path, model_id: str) -> str | None:
    """Best-effort pinned model revision from the HF cache refs."""
    slug = "models--" + model_id.replace("/", "--")
    ref = hf_home / "hub" / slug / "refs" / "main"
    try:
        text = ref.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return text or None


def mapping_proof(
    admitted: list[tuple[str, str]],
    observed: list[str],
    corpus_dir: Path,
) -> tuple[dict, str]:
    """Path map + both-side path+SHA diff. Returns (proof, diff_digest)."""
    if not isinstance(observed, list) or any(not isinstance(name, str) for name in observed):
        raise AdapterError("Semble observed files must be a path list")
    if len(observed) != len(set(observed)):
        raise AdapterError("Semble observed files contain a duplicate path")
    corpus_root = corpus_dir.resolve()
    for name in observed:
        if (
            not name
            or name.startswith("/")
            or "\\" in name
            or any(part in ("", ".", "..") for part in name.split("/"))
        ):
            raise AdapterError(f"unsafe Semble observed path: {name!r}")
        target = corpus_dir / name
        if corpus_root not in target.resolve().parents:
            raise AdapterError(f"Semble observed path escapes the corpus: {name!r}")
        current = corpus_dir
        for part in name.split("/"):
            current = current / part
            if current.is_symlink():
                raise AdapterError(f"Semble observed path uses a symlink: {name!r}")
    admitted_names = [name for name, _ in admitted]
    admitted_set = set(admitted_names)
    observed_set = set(observed)
    quanta_side = sorted(
        ({"path": n, "file_sha256": s} for n, s in admitted),
        key=lambda row: row["path"],
    )
    semble_side = []
    for name in sorted(observed_set):
        target = corpus_dir / name
        try:
            data = target.read_bytes() if target.is_file() else None
        except OSError:
            data = None
        semble_side.append(
            {
                "path": name,
                "file_sha256": hashlib.sha256(data).hexdigest() if data is not None else None,
                "readable": data is not None,
            }
        )
    observed_by_path = {row["path"]: row for row in semble_side}
    per_file = []
    mismatched = []
    for name, sha in sorted(admitted):
        if name not in observed_set:
            status = "skipped"
        else:
            side = observed_by_path[name]
            if not side["readable"]:
                status = "unreadable"
            elif side["file_sha256"] != sha:
                status = "hash_mismatch"
            else:
                status = "indexed"
            if status in ("unreadable", "hash_mismatch"):
                mismatched.append(name)
        per_file.append({"path": name, "file_sha256": sha, "status": status})
    for name in sorted(observed_set - admitted_set):
        per_file.append({"path": name, "file_sha256": None, "status": "extra"})
    proof = {
        "path_map": [
            {"semble_path": name, "canonical_path": name} for name in sorted(observed_set)
        ],
        "quanta_side": quanta_side,
        "semble_side": semble_side,
        "per_file": sorted(per_file, key=lambda row: row["path"]),
        "admitted_count": len(admitted_set),
        "observed_count": len(observed_set),
        "skipped": sorted(admitted_set - observed_set),
        "extra": sorted(observed_set - admitted_set),
        "mismatched": mismatched,
    }
    diff_digest = digest(
        json.dumps(
            {"quanta_side": quanta_side, "semble_side": semble_side},
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
    )
    proof["diff_digest"] = diff_digest
    return proof, diff_digest


def count_tokens(text: str) -> int:
    return len(TOKEN_RE.findall(text))


def _source_line_offsets(file_lines: dict[str, list[bytes]]) -> dict[str, list[int]]:
    offsets_by_path = {}
    for path, lines in file_lines.items():
        offsets = [0]
        for line in lines:
            offsets.append(offsets[-1] + len(line))
        offsets_by_path[path] = offsets
    return offsets_by_path


def normalize_results(
    pack,
    native,
    latencies,
    file_shas,
    file_lines,
    contract,
    route,
    profile,
    *,
    indexed_chunks=None,
    source_line_offsets=None,
    verified_blocks=None,
):
    """One canonical per-query normalization owner; capture envelopes are separate."""
    if not isinstance(contract, dict) or pack.get("comparison_contract") != contract:
        raise AdapterError("pack comparison contract differs from the capture contract")
    top_k = contract.get("top_k")
    if not isinstance(top_k, int) or isinstance(top_k, bool) or top_k <= 0:
        raise AdapterError("capture contract top_k must be a positive integer")
    if not isinstance(native, list):
        raise AdapterError("Semble native rows must be a list")
    file_mode = profile.get("mode") == "lexical-file"
    if file_mode and (type(indexed_chunks) is not int or indexed_chunks <= 0):
        raise AdapterError("Semble file collection lacks indexed chunk count")
    line_offsets = (
        _source_line_offsets(file_lines) if source_line_offsets is None else source_line_offsets
    )
    if verified_blocks is None:
        verified_blocks = {}
    results = []

    def append_result(
        row: dict, *, matched_chunks: int | None = None, matched_files: int | None = None
    ):
        if file_mode:
            row.update(
                rank_unit="distinct_file",
                ordering="score_desc_native_tiebreak",
                score_evidence="semble_bm25_score_v1",
            )
            if matched_chunks is not None and matched_files is not None:
                row["file_collection"] = {
                    "indexed_chunks": indexed_chunks,
                    "matched_chunks": matched_chunks,
                    "matching_files": matched_files,
                }
        results.append(row)

    by_task: dict[str, object] = {}
    expected_task_ids = {task["task_id"] for task in pack["tasks"]}
    for row in native:
        if not isinstance(row, dict) or set(row) != {"task_id", "results"}:
            raise AdapterError("Semble native row must hold exactly task_id and results")
        task_key = row["task_id"]
        if not isinstance(task_key, str) or not task_key:
            raise AdapterError("Semble native row lacks a task_id")
        if task_key not in expected_task_ids:
            raise AdapterError(f"Semble emitted an unexpected native task: {task_key}")
        if task_key in by_task:
            raise AdapterError(f"Semble emitted a duplicate native row: {task_key}")
        by_task[task_key] = row["results"]
    if not isinstance(latencies, dict) or any(
        task_id not in expected_task_ids for task_id in latencies
    ):
        raise AdapterError("Semble latencies contain an unexpected task or invalid mapping")
    for task in pack["tasks"]:
        task_id = task["task_id"]
        submitted_sha = hashlib.sha256(task["query"].encode()).hexdigest()
        query_identity = {
            "original_query_sha256": submitted_sha,
            "submitted_query_sha256": submitted_sha,
        }
        if task_id not in by_task:
            append_result(
                {
                    "task_id": task_id,
                    "route": route,
                    "status": "error",
                    "query_identity": query_identity,
                    "timings": {"query_latency_ms": None},
                    "candidates": [],
                    "error": {
                        "code": "semble_missing_query",
                        "message": "Semble emitted no row for this query",
                    },
                }
            )
            continue
        samples = latencies.get(task_id, [])
        latency = samples[0] if samples else None
        hits = by_task[task_id]
        if not isinstance(hits, list):
            raise AdapterError(f"Semble native row is not a list: {task_id}")
        if len(hits) > (indexed_chunks if file_mode else top_k):
            bound = "indexed chunk count" if file_mode else "top_k"
            raise AdapterError(f"Semble exceeded {bound} for {task_id}")
        if not hits:
            append_result(
                {
                    "task_id": task_id,
                    "route": route,
                    "status": "abstained",
                    "query_identity": query_identity,
                    "candidates": [],
                    "timings": {"query_latency_ms": latency},
                    "error": None,
                },
                matched_chunks=0,
                matched_files=0,
            )
            continue
        candidates = []
        seen_spans = set()
        seen_files = set()
        previous_score = None
        hit_error = None
        for hit in hits:
            # Per-hit content failures become error rows (pair-incomplete at
            # verdict) instead of silently clamped spans or fabricated bytes.
            if isinstance(hit, dict):
                path = hit.get("file_path")
                start = hit.get("start_line")
                end = hit.get("end_line")
                score = hit.get("score")
            else:
                path = start = end = score = None
            if file_mode:
                if not is_finite_json_number(score) or score <= 0:
                    raise AdapterError(f"Semble file collection has invalid BM25 score: {task_id}")
                if previous_score is not None and score > previous_score:
                    raise AdapterError(
                        f"Semble file collection is not in native score order: {task_id}"
                    )
                previous_score = score
            if not isinstance(path, str) or path not in file_shas:
                hit_error = {
                    "code": "semble_hit_outside_universe",
                    "message": f"Semble hit outside admitted universe: {path!r}",
                }
                break
            if type(start) is not int or type(end) is not int or start < 1 or end < start:
                hit_error = {
                    "code": "semble_hit_bad_span",
                    "message": f"Semble hit has a bad span: {path}:{start}-{end}",
                }
                break
            cache_key = (path, start, end)
            verified = verified_blocks.get(cache_key)
            if verified is None:
                lines = file_lines[path]
                if end > len(lines):
                    hit_error = {
                        "code": "semble_hit_beyond_eof",
                        "message": f"Semble hit spans beyond EOF: {path}:{start}-{end}",
                    }
                    break
                block = b"".join(lines[start - 1 : end])
                try:
                    text = block.decode("utf-8")
                except UnicodeDecodeError:
                    hit_error = {
                        "code": "semble_hit_not_utf8",
                        "message": f"Semble hit block is not UTF-8: {path}",
                    }
                    break
                tokens = count_tokens(text)
                if tokens == 0:
                    hit_error = {
                        "code": "semble_hit_no_tokens",
                        "message": f"Semble hit holds no tokens: {path}:{start}-{end}",
                    }
                    break
                start_byte = line_offsets[path][start - 1]
                verified = (
                    start_byte,
                    start_byte + len(block),
                    hashlib.sha256(block).hexdigest(),
                    tokens,
                )
                verified_blocks[cache_key] = verified
            start_byte, end_byte, block_sha256, tokens = verified
            span = (path, start_byte, end_byte)
            # Native hits remain in the raw capture. Prove every hit before
            # either source-span collapse or first-occurrence file projection.
            if file_mode:
                if path in seen_files:
                    continue
                seen_files.add(path)
                if len(candidates) == top_k:
                    continue
            else:
                if span in seen_spans:
                    continue
                seen_spans.add(span)
            candidate = {
                "path": path,
                "start_byte": start_byte,
                "end_byte": end_byte,
                "start_line": start,
                "end_line": end,
                "file_sha256": file_shas[path],
                "block_sha256": block_sha256,
                "tokens": tokens,
                "rank": len(candidates) + 1,
            }
            if file_mode:
                candidate["score"] = score
            candidates.append(candidate)
        if hit_error is not None:
            append_result(
                {
                    "task_id": task_id,
                    "route": route,
                    "status": "error",
                    "query_identity": query_identity,
                    "timings": {"query_latency_ms": latency},
                    "candidates": [],
                    "error": hit_error,
                }
            )
            continue
        append_result(
            {
                "task_id": task_id,
                "route": route,
                "status": "success",
                "query_identity": query_identity,
                "candidates": candidates,
                "timings": {"query_latency_ms": latency},
                "error": None,
            },
            matched_chunks=len(hits),
            matched_files=len(seen_files) if file_mode else None,
        )
    return results


def assemble_record(
    pack,
    pack_sha256,
    results,
    native,
    run_id,
    blinding,
    isolation_method,
    access_block_log,
    model,
    model_revision,
    route,
    profile,
    capture_id,
    receipt_digest,
    worker_digest,
):
    """Assemble provenance once around already completed normalized responses."""
    contract = pack["comparison_contract"]
    if not isinstance(capture_id, str) or not capture_id.strip():
        raise AdapterError("capture_id must be a nonempty string")
    for label, value in (("receipt_digest", receipt_digest), ("worker_digest", worker_digest)):
        if (
            not isinstance(value, str)
            or len(value) != 64
            or any(c not in "0123456789abcdef" for c in value)
        ):
            raise AdapterError(f"{label} must be a lowercase sha256")
    ordered_native = sorted(native, key=lambda row: str(row.get("task_id")))
    return {
        "schema_version": 5,
        "query_pack_sha256": pack_sha256,
        "comparison_contract": contract,
        "runner": {
            "name": f"semble-adapter/{profile['mode']}",
            "revision": run_id,
            "run_id": run_id,
            "tokenizer": TOKENIZER,
            "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": blinding,
            "isolation_method": isolation_method,
            "access_block_log": access_block_log,
        },
        "captures": {
            capture_id: {
                "system": "semble",
                "chunk_strategy": "semble_native",
                "chunk_config": {},
                "runner_binary": {"name": "semble-worker", "digest": worker_digest},
                "searchd_binary": None,
                "generation": 0,
                "receipt_digest": receipt_digest,
                "activation_digest": digest(canonical(ordered_native)),
                "model": model,
                "model_revision": model_revision,
                "execution_profile": profile,
                "execution_profile_sha256": digest(canonical(profile)),
            }
        },
        "route_provenance": {route: {"capture_id": capture_id}},
        "results": results,
    }


def normalize_record(
    pack: dict,
    pack_sha256: str,
    native: list[dict],
    latencies: dict[str, list[float]],
    repo: Path,
    file_shas: dict[str, str],
    file_lines: dict[str, list[bytes]],
    contract: dict,
    run_id: str,
    blinding: str,
    isolation_method: str,
    access_block_log: str,
    model: str,
    model_revision: str,
    route: str,
    profile: dict,
    capture_id: str,
    receipt_digest: str,
    worker_digest: str,
    *,
    indexed_chunks: int | None = None,
) -> dict:
    """Replay native captures through the same row and envelope owners."""
    results = normalize_results(
        pack,
        native,
        latencies,
        file_shas,
        file_lines,
        contract,
        route,
        profile,
        indexed_chunks=indexed_chunks,
    )
    return assemble_record(
        pack,
        pack_sha256,
        results,
        native,
        run_id,
        blinding,
        isolation_method,
        access_block_log,
        model,
        model_revision,
        route,
        profile,
        capture_id,
        receipt_digest,
        worker_digest,
    )


def cmd_check(args: argparse.Namespace) -> int:
    try:
        report = check_semble_env(Path(args.python))
    except AdapterError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


def cmd_run(args: argparse.Namespace) -> int:
    try:
        return run_adapter(args)
    except AdapterError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


def validate_worker_phase_timings(
    native_payload: dict, *, protocol: bool
) -> tuple[dict[str, int | float], int | float]:
    """Reject non-finite worker timings before emitting any normalized artifact."""
    phases = {
        "discovery": native_payload.get("discovery_ms"),
        "model_provider_prepare": native_payload.get("model_provider_prepare_ms"),
        "index": native_payload.get("semble_index_ms"),
        "warmup": native_payload.get("warmup_ms"),
    }
    if protocol:
        phases["cold_query"] = native_payload.get("cold_query_ms")
        phases["warm_query"] = native_payload.get("protocol_warm_query_ms")
    else:
        phases["first_query"] = native_payload.get("first_query_ms")
        phases["warm_query"] = native_payload.get("warm_query_ms")

    def finite_nonnegative(value: object) -> bool:
        return is_finite_json_number(value) and value >= 0

    if any(not finite_nonnegative(value) for value in phases.values()):
        raise AdapterError("Semble worker omitted finite nonnegative phase timings")
    phase_sum = sum(phases.values())
    total = native_payload.get("worker_total_ms")
    if not finite_nonnegative(phase_sum) or not finite_nonnegative(total) or total < phase_sum:
        raise AdapterError("Semble worker total timing is inconsistent with phases")
    phases["unattributed"] = total - phase_sum
    return phases, total


def run_completed_worker(
    command, *, env, timeout_secs, tasks, top_k, route, normalize_response, stderr_path
):
    """One resident worker; one parent clock covers request through normalized status.

    Query execution, native JSON materialization, pipe transfer, decoding and
    parent normalization all occur before end_ns. Worker phase timestamps are
    never subtracted from the parent's clock. Startup/indexing precede requests.
    """
    origin_ns = time.monotonic_ns()
    deadline = time.monotonic() + timeout_secs
    completed_rows = {}
    completed_output_digests = {}
    with open(stderr_path, "w", encoding="utf-8") as stderr:
        process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=stderr,
            env=env,
            text=True,
            bufsize=1,
        )
        messages = queue.Queue(maxsize=2)
        stopped = threading.Event()

        def read_messages():
            while not stopped.is_set():
                try:
                    raw = process.stdout.readline(64 * 1024 * 1024 + 1)
                except (OSError, ValueError) as exc:
                    raw = exc
                while not stopped.is_set():
                    try:
                        messages.put(raw, timeout=0.1)
                        break
                    except queue.Full:
                        continue
                if not raw or isinstance(raw, Exception):
                    break

        reader = threading.Thread(target=read_messages, daemon=True)
        reader.start()
        try:

            def receive():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AdapterError("Semble completed-response worker timed out")
                try:
                    raw = messages.get(timeout=remaining)
                except queue.Empty as exc:
                    raise AdapterError("Semble completed-response worker timed out") from exc
                if isinstance(raw, Exception):
                    raise AdapterError("Semble completed-response protocol read failed") from raw
                if not raw or len(raw) > 64 * 1024 * 1024 or not raw.endswith("\n"):
                    raise AdapterError("Semble worker omitted a bounded complete protocol message")
                try:
                    message = json.loads(raw)
                except ValueError as exc:
                    raise AdapterError("Semble worker emitted invalid protocol JSON") from exc
                if not isinstance(message, dict):
                    raise AdapterError("Semble worker protocol message must be an object")
                return message

            while True:
                ready = receive()
                if ready.get("kind") == "finished":
                    break
                if ready.get("kind") != "request_ready" or ready.get("task_id") not in tasks:
                    raise AdapterError("Semble worker emitted an unexpected request")
                task_id = ready["task_id"]
                start_ns = time.monotonic_ns() - origin_ns
                request = {"task_id": task_id, "query": tasks[task_id], "top_k": top_k}
                process.stdin.write(json.dumps(request) + "\n")
                process.stdin.flush()
                response = receive()
                if response.get("kind") != "response" or response.get("task_id") != task_id:
                    raise AdapterError("Semble worker response differs from the pending request")
                row = normalize_response(task_id, response.get("results"), ready["indexed_chunks"])
                required_output = dict(row)
                required_output.pop("timings")
                output_bytes = len(canonical(required_output))
                end_ns = time.monotonic_ns() - origin_ns
                # Verification remains outside the completed-response timer.
                output_sha256 = completed_output_sha256(required_output)
                previous = completed_output_digests.setdefault(task_id, output_sha256)
                if previous != output_sha256:
                    raise AdapterError("Semble completed response changed between repetitions or phases")
                observation = {
                    "task_id": task_id,
                    "route": route,
                    "phase": ready["phase"],
                    "iteration": ready["iteration"],
                    "start_ns": start_ns,
                    "end_ns": end_ns,
                    "status": row["status"],
                    "output_bytes": output_bytes,
                    "output_sha256": output_sha256,
                }
                row["timings"]["query_latency_ms"] = (end_ns - start_ns) / 1e6
                if ready["phase"] == "measured" and ready["iteration"] == 0:
                    if task_id in completed_rows:
                        raise AdapterError("Semble worker repeated a first completed response")
                    completed_rows[task_id] = row
                process.stdin.write(json.dumps(observation) + "\n")
                process.stdin.flush()
            process.stdin.close()
            process.wait(timeout=max(deadline - time.monotonic(), 0.01))
            if process.returncode != 0:
                raise AdapterError(f"Semble completed-response worker exited {process.returncode}")
        except BaseException:
            process.kill()
            process.wait()
            raise
        finally:
            stopped.set()
            reader.join(timeout=1)
            if not reader.is_alive():
                process.stdout.close()
            if not process.stdin.closed:
                process.stdin.close()
    if set(completed_rows) != set(tasks):
        raise AdapterError("Semble worker did not complete the full task inventory")
    completed = subprocess.CompletedProcess(
        command, process.returncode, "", Path(stderr_path).read_text()
    )
    return completed, [completed_rows[task_id] for task_id in tasks]


def run_adapter(args: argparse.Namespace) -> int:
    repo = Path(args.repo)
    out_root = Path(args.output_root)
    commit, manifest_rows = load_manifest(Path(args.manifest))
    if args.materialized_corpus:
        repo = verify_materialized_corpus(repo, manifest_rows)
    else:
        try:
            repo = verify_repo(repo, commit)
        except ValueError as exc:
            raise AdapterError(f"pinned repository proof failed: {exc}") from exc
    if repo in out_root.resolve().parents or out_root.resolve() == repo:
        raise AdapterError("output root must be outside the frozen repository")
    cache_root = Path(args.cache_root)
    if repo in cache_root.resolve().parents or cache_root.resolve() == repo:
        raise AdapterError("cache root must be outside the frozen repository")
    if not cache_root.is_dir():
        raise AdapterError("cache root must be an existing directory")
    if out_root.exists():
        raise AdapterError(f"output root already exists (refusing reuse): {out_root}")
    out_root.mkdir(parents=True)
    pack = load_query_pack(Path(args.query_pack))
    if pack.get("repository_commit") != commit:
        raise AdapterError("manifest commit differs from query-pack commit")
    top_k = _int(args.top_k, "top_k")
    if top_k <= 0:
        raise AdapterError("top_k must be positive")
    if pack["comparison_contract"]["top_k"] != top_k:
        raise AdapterError("CLI top_k differs from the query-pack comparison contract")
    if args.blinding not in ("isolated", "attested"):
        raise AdapterError("blinding must be isolated or attested")

    env_report = check_semble_env(Path(args.python))
    try:
        external_lock = Path(args.lockfile).read_bytes()
    except OSError as exc:
        raise AdapterError(f"cannot read the external lockfile: {exc}") from exc
    lockfile_digest = verify_lockfile(
        external_lock, args.lockfile_sha256, env_report["observed_freeze"]
    )
    semble_version = env_report["semble_version"]
    lockfile = out_root / "lockfile.txt"
    lockfile.write_bytes(external_lock)
    assert sha_file(lockfile) == lockfile_digest

    corpus_dir = out_root / "corpus"
    admitted_rows, max_bytes = build_isolated_corpus(repo, manifest_rows, corpus_dir)
    # Semble skips files above its size cap; raise the cap to cover the
    # admitted universe explicitly and record the override (never silent).
    max_file_bytes = max(max_bytes + 1024, 1024 * 1024)

    worker_path = out_root / "worker.py"
    worker_path.write_text(WORKER_TEMPLATE, encoding="utf-8")
    worker_digest = sha_file(worker_path)
    repetitions = _int(args.repetitions, "repetitions")
    warmup_passes = _int(args.warmup_passes, "warmup_passes")
    seed = _int(args.seed, "seed")
    if repetitions <= 0 or warmup_passes < 0:
        raise AdapterError("repetitions must be positive and warmup_passes non-negative")
    task_ids = [task["task_id"] for task in pack["tasks"]]
    query_protocol = None
    if args.query_protocol is not None:
        query_protocol = validate_query_protocol(read_json(Path(args.query_protocol)), task_ids)
        if len(query_protocol["warmup_schedules"]) != warmup_passes:
            raise AdapterError("query protocol warmup count differs from CLI")
        if len(query_protocol["measurement_schedules"]) != repetitions:
            raise AdapterError("query protocol repetition count differs from CLI")
        if query_protocol["seed"] != seed:
            raise AdapterError("query protocol seed differs from CLI")
    profile_mode = str(getattr(args, "semble_profile", "native-default"))
    requested_alpha = getattr(args, "alpha", None)
    profile = execution_profile(profile_mode, requested_alpha)
    alpha = profile["alpha"]
    spec = {
        "corpus_dir": str(corpus_dir),
        "tasks": [{"task_id": task["task_id"], "query": task["query"]} for task in pack["tasks"]],
        "top_k": top_k,
        "seed": seed,
        "warmup_passes": warmup_passes,
        "repetitions": repetitions,
        "query_protocol": query_protocol,
        "semble_profile": profile_mode,
        "alpha": alpha,
        "execution_profile_sha256": digest(canonical(profile)),
    }
    spec_path = out_root / "spec.json"
    spec_path.write_text(json.dumps(spec, indent=2, sort_keys=True), encoding="utf-8")
    native_path = out_root / "native.json"
    model_id = args.model_id
    model_revision, source_model_asset = resolve_model_revision(
        cache_root / "hf", model_id, args.model_revision
    )
    materialized_hf = out_root / "model-cache" / "hf"
    model_cache_manifest = materialize_model_cache(
        cache_root / "hf", materialized_hf, model_id, model_revision
    )
    if model_asset_digest(materialized_hf, model_id, model_revision) != source_model_asset:
        raise AdapterError("materialized model snapshot differs from the pinned source cache")
    model_cache_path = out_root / "model-cache-manifest.json"
    model_cache_path.write_text(
        json.dumps(model_cache_manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    env = dict(os.environ)
    env["SPEC_JSON"] = str(spec_path)
    env["NATIVE_JSON"] = str(native_path)
    runtime_cache = out_root / "semble-runtime-cache"
    runtime_cache.mkdir()
    env["SEMBLE_CACHE_LOCATION"] = str(runtime_cache)
    env["HF_HOME"] = str(materialized_hf)
    env["HF_HUB_OFFLINE"] = "1"
    env["TRANSFORMERS_OFFLINE"] = "1"
    env["SEMBLE_MODEL_NAME"] = model_id
    env["SEMBLE_MAX_FILE_BYTES"] = str(max_file_bytes)
    file_shas: dict[str, str] = {}
    file_lines: dict[str, list[bytes]] = {}
    for name, sha in admitted_rows:
        data = (repo / name).read_bytes()
        if hashlib.sha256(data).hexdigest() != sha:
            raise AdapterError(f"pinned source drifted during the run: {name}")
        file_shas[name] = sha
        file_lines[name] = data.splitlines(keepends=True)

    tasks_by_id = {task["task_id"]: task for task in pack["tasks"]}
    source_line_offsets = _source_line_offsets(file_lines)
    verified_blocks = {}

    def normalize_response(task_id, hits, indexed_chunks):
        single_pack = dict(pack, tasks=[tasks_by_id[task_id]])
        normalized = normalize_results(
            single_pack,
            [{"task_id": task_id, "results": hits}],
            {},
            file_shas,
            file_lines,
            pack["comparison_contract"],
            args.route,
            profile,
            indexed_chunks=indexed_chunks,
            source_line_offsets=source_line_offsets,
            verified_blocks=verified_blocks,
        )
        return normalized[0]

    try:
        completed, completed_rows = run_completed_worker(
            [str(Path(args.python)), str(worker_path)],
            timeout_secs=_int(args.timeout_secs, "timeout_secs"),
            env=env,
            tasks={task_id: task["query"] for task_id, task in tasks_by_id.items()},
            top_k=top_k,
            route=args.route,
            normalize_response=normalize_response,
            stderr_path=out_root / "worker.stderr.log",
        )
    except subprocess.TimeoutExpired as exc:
        raise AdapterError(f"Semble worker timed out: {exc}") from exc
    except (OSError, subprocess.CalledProcessError) as exc:
        tail = ""
        if isinstance(exc, subprocess.CalledProcessError):
            (out_root / "worker.stdout.log").write_text(exc.stdout or "", encoding="utf-8")
            (out_root / "worker.stderr.log").write_text(exc.stderr or "", encoding="utf-8")
            tail = (exc.stderr or "")[-2000:]
        raise AdapterError(f"Semble worker failed: {exc}\nstderr tail: {tail}") from exc
    (out_root / "worker.stdout.log").write_text(completed.stdout or "", encoding="utf-8")
    (out_root / "worker.stderr.log").write_text(completed.stderr or "", encoding="utf-8")
    native_payload = read_json(native_path)
    if not isinstance(native_payload, dict):
        raise AdapterError("Semble native output must be an object")
    if native_payload.get("configured_model_name") != model_id:
        raise AdapterError("Semble worker model configuration differs from requested model")
    validate_native_profile_report(
        native_payload,
        profile_mode,
        alpha,
        expected_query_sha256={task["task_id"]: task["query_sha256"] for task in pack["tasks"]},
    )
    observed = native_payload.get("observed_files", [])
    proof, diff_digest = mapping_proof(admitted_rows, observed, corpus_dir)
    (out_root / "mapping-proof.json").write_text(
        json.dumps(proof, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    if proof["skipped"] or proof["extra"] or proof["mismatched"]:
        raise AdapterError(
            "common-universe pair ineligible: "
            f"skipped={proof['skipped']} extra={proof['extra']} "
            f"mismatched={proof['mismatched']} "
            f"(see {out_root / 'mapping-proof.json'})"
        )

    # Reverify the source after every measured response used the pinned bytes.
    for name, sha in admitted_rows:
        data = (repo / name).read_bytes()
        if hashlib.sha256(data).hexdigest() != sha:
            raise AdapterError(f"pinned source drifted during the run: {name}")

    pack_canonical = json.dumps(
        json.loads(Path(args.query_pack).read_text(encoding="utf-8")),
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    )
    pack_sha256 = digest(pack_canonical.encode("utf-8"))
    model_asset = model_asset_digest(materialized_hf, model_id, model_revision)
    if model_asset != source_model_asset:
        raise AdapterError("model snapshot changed while the worker was running")
    record = assemble_record(
        pack,
        pack_sha256,
        completed_rows,
        native_payload.get("native", []),
        args.run_id,
        args.blinding,
        args.isolation_method,
        args.access_block_log,
        model_id,
        model_revision,
        args.route,
        profile,
        args.run_id,
        diff_digest,
        worker_digest,
    )
    phase_values, worker_total_ms = validate_worker_phase_timings(
        native_payload, protocol=query_protocol is not None
    )
    phase_boundaries_ns = native_payload.get("phase_boundaries_ns")
    if not isinstance(phase_boundaries_ns, dict):
        raise AdapterError("Semble worker omitted monotonic phase boundaries")
    record_path = out_root / "record.json"
    record_path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    phase_metrics = {
        "schema_version": 2,
        "system": "semble",
        "profile": profile_mode,
        "requested_alpha": native_payload.get("requested_alpha"),
        "rerank_applied": native_payload.get("rerank_applied"),
        "lane_call_counts": native_payload.get("lane_call_counts"),
        "execution_events_sha256": native_payload.get("execution_events_sha256"),
        "function_identity": native_payload.get("function_identity"),
        "observed_wrapped_call_ns": native_payload.get("observed_wrapped_call_ns"),
        "timing_layer": "worker_monotonic_wall_v1",
        "query_timing": native_payload["query_timing"],
        "strategy": "native",
        "record_sha256": sha_file(record_path),
        "worker_sha256": worker_digest,
        "task_count": len(pack["tasks"]),
        "route_count": 1,
        "file_count": native_payload.get("stats", {}).get("indexed_files"),
        "chunk_count": native_payload.get("stats", {}).get("total_chunks"),
        "query_schedule": native_payload.get("query_schedule"),
        "warmup_passes": warmup_passes,
        "measurement_repetitions": repetitions,
        "phases_ms": phase_values,
        "phase_boundaries_ns": phase_boundaries_ns,
        "total_ms": worker_total_ms,
    }
    if query_protocol is not None:
        phase_metrics["query_protocol"] = query_protocol
        phase_metrics["warm_latencies_ms"] = {args.route: native_payload.get("latencies_ms")}
        phase_metrics["cold_latencies_ms"] = {args.route: native_payload.get("cold_latency_ms")}
    (out_root / "phase-metrics.json").write_text(
        json.dumps(phase_metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    manifest_out = {
        "semble_version": semble_version,
        "profile": profile,
        "requested_alpha": native_payload.get("requested_alpha"),
        "actual_alpha_by_task": native_payload.get("actual_alpha_by_task"),
        "rerank_applied": native_payload.get("rerank_applied"),
        "lane_call_counts": native_payload.get("lane_call_counts"),
        "execution_events_sha256": native_payload.get("execution_events_sha256"),
        "function_identity": native_payload.get("function_identity"),
        "observed_wrapped_call_ns": native_payload.get("observed_wrapped_call_ns"),
        "semble_python": str(Path(args.python)),
        "interpreter": env_report["interpreter"],
        "worker_digest": worker_digest,
        "lockfile_digest": lockfile_digest,
        "observed_freeze_digest": env_report["observed_freeze_sha256"],
        "installed_distribution": env_report["installed_distribution"],
        "model_id": model_id,
        "model_revision": model_revision,
        "model_asset_digest": model_asset,
        "model_cache_manifest_digest": sha_file(model_cache_path),
        "record_digest": sha_file(record_path),
        "timing_layer": "library",
        "semble_index_ms": native_payload.get("semble_index_ms"),
        "index_stats": native_payload.get("stats"),
        "repetitions": repetitions,
        "warmup_passes": warmup_passes,
        "seed": seed,
        "query_protocol_sha256": (query_protocol["sha256"] if query_protocol is not None else None),
        "semble_max_file_bytes": max_file_bytes,
        "path_sha_diff_digest": diff_digest,
        "top_k": top_k,
    }
    (out_root / "adapter-manifest.json").write_text(
        json.dumps(manifest_out, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(manifest_out, indent=2, sort_keys=True))
    return 0


def model_asset_digest(hf_home: Path, model_id: str, revision: str) -> str:
    """Digest logical paths and bytes of one pinned model snapshot."""
    slug = "models--" + model_id.replace("/", "--")
    model_root = (hf_home / "hub" / slug).resolve()
    snapshot = model_root / "snapshots" / revision
    if not snapshot.is_dir():
        raise AdapterError(f"model snapshot unavailable in HF cache: {model_id}@{revision}")
    digestor = hashlib.sha256()
    members = []
    for path in sorted(snapshot.rglob("*")):
        try:
            target = path.resolve(strict=True)
        except (OSError, RuntimeError) as exc:
            raise AdapterError(f"unsafe model snapshot member {path}: {exc}") from exc
        if model_root != target and model_root not in target.parents:
            raise AdapterError(f"model snapshot member escapes model root: {path}")
        if target.is_file():
            members.append((path, target))
        elif not target.is_dir():
            raise AdapterError(f"model snapshot member is not regular: {path}")
    if not members:
        raise AdapterError(f"model snapshot holds no files: {model_id}@{revision}")
    for path, target in members:
        try:
            data = target.read_bytes()
        except OSError as exc:
            raise AdapterError(f"cannot read model asset {path}: {exc}") from exc
        digestor.update(path.relative_to(snapshot).as_posix().encode("utf-8"))
        digestor.update(b"\0")
        digestor.update(data)
        digestor.update(b"\0")
    return digestor.hexdigest()


def materialize_model_cache(
    source_hf_home: Path,
    destination_hf_home: Path,
    model_id: str,
    revision: str,
) -> dict:
    """Copy one pinned HF snapshot into a closed, symlink-free cache."""
    slug = "models--" + model_id.replace("/", "--")
    source_repo = (source_hf_home / "hub" / slug).resolve()
    snapshot = source_repo / "snapshots" / revision
    ref = source_repo / "refs" / "main"
    if not snapshot.is_dir() or not ref.is_file():
        raise AdapterError(f"model cache lacks snapshot/ref for {model_id}@{revision}")
    try:
        ref_revision = ref.read_text(encoding="utf-8").strip()
    except OSError as exc:
        raise AdapterError(f"cannot read model cache ref: {exc}") from exc
    if ref_revision != revision:
        raise AdapterError(
            f"model cache ref drift: refs/main={ref_revision!r}, pinned={revision!r}"
        )
    destination_repo = destination_hf_home / "hub" / slug
    destination_resolved = destination_repo.resolve()
    if source_repo == destination_resolved or source_repo in destination_resolved.parents:
        raise AdapterError("materialized model cache must be outside the source model root")
    if destination_repo.exists():
        raise AdapterError(f"materialized model cache already exists: {destination_repo}")
    destination_snapshot = destination_repo / "snapshots" / revision
    destination_snapshot.mkdir(parents=True)
    members = []
    asset_digestor = hashlib.sha256()
    for logical in sorted(snapshot.rglob("*")):
        if logical.is_symlink():
            try:
                linked_target = logical.resolve(strict=True)
            except (OSError, RuntimeError) as exc:
                raise AdapterError(f"unsafe model cache link {logical}: {exc}") from exc
            if linked_target.is_dir():
                raise AdapterError(f"model cache directory symlink is unsupported: {logical}")
        elif logical.is_dir():
            continue
        relative = logical.relative_to(snapshot)
        try:
            target = logical.resolve(strict=True)
        except (OSError, RuntimeError) as exc:
            raise AdapterError(f"unsafe model cache link {logical}: {exc}") from exc
        if source_repo != target and source_repo not in target.parents:
            raise AdapterError(f"model cache link escapes model root: {logical} -> {target}")
        if not target.is_file():
            raise AdapterError(f"model cache member is not a regular file: {logical}")
        try:
            data = target.read_bytes()
        except OSError as exc:
            raise AdapterError(f"cannot read model cache member {logical}: {exc}") from exc
        output = destination_snapshot / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(data)
        asset_digestor.update(relative.as_posix().encode("utf-8"))
        asset_digestor.update(b"\0")
        asset_digestor.update(data)
        asset_digestor.update(b"\0")
        members.append(
            {
                "path": relative.as_posix(),
                "sha256": digest(data),
                "size": len(data),
            }
        )
    if not members:
        raise AdapterError("model snapshot holds no materializable files")
    destination_ref = destination_repo / "refs" / "main"
    destination_ref.parent.mkdir(parents=True)
    destination_ref.write_text(revision + "\n", encoding="utf-8")
    manifest = {
        "schema_version": 1,
        "model_id": model_id,
        "revision": revision,
        "ref": {"name": "main", "revision": revision},
        "members": members,
        "model_asset_digest": asset_digestor.hexdigest(),
    }
    manifest["snapshot_digest"] = digest(canonical(manifest))
    return manifest


def resolve_model_revision(hf_home: Path, model_id: str, pinned: str | None) -> tuple[str, str]:
    """Return (revision, model_asset_digest) for the pinned model snapshot."""
    observed = read_hf_revision(hf_home, model_id)
    if (
        observed is None
        or len(observed) != 40
        or any(c not in "0123456789abcdef" for c in observed)
    ):
        raise AdapterError(f"model revision unavailable or invalid in HF cache: {model_id}")
    if pinned and observed != pinned:
        raise AdapterError(f"model revision drift: pinned {pinned} but cache holds {observed}")
    revision = pinned or observed
    return revision, model_asset_digest(hf_home, model_id, revision)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check", help="verify the pinned Semble env")
    check.add_argument("--python", required=True)
    run = sub.add_parser("run", help="run Semble and emit record + mapping proof")
    run.add_argument("--repo", required=True)
    run.add_argument("--manifest", required=True)
    run.add_argument("--query-pack", required=True)
    run.add_argument("--top-k", required=True)
    run.add_argument("--python", required=True)
    run.add_argument("--lockfile", required=True)
    run.add_argument("--lockfile-sha256", required=True)
    run.add_argument("--cache-root", required=True)
    run.add_argument("--output-root", required=True)
    run.add_argument("--model-id", default=DEFAULT_MODEL_ID)
    run.add_argument("--model-revision", default=None)
    run.add_argument("--route", default="semble-hybrid")
    run.add_argument("--run-id", required=True)
    run.add_argument("--blinding", required=True)
    run.add_argument("--isolation-method", required=True)
    run.add_argument("--access-block-log", required=True)
    run.add_argument("--seed", default="0")
    run.add_argument("--warmup-passes", default="1")
    run.add_argument("--repetitions", default="1")
    run.add_argument("--query-protocol", default=None)
    run.add_argument(
        "--semble-profile",
        default="native-default",
        choices=SEMBLE_PROFILES,
        help="RBR-03 comparison profile; every phase dispatches through one shared path",
    )
    run.add_argument(
        "--alpha",
        type=float,
        default=None,
        help="explicit fusion weight for --semble-profile hybrid-no-rerank",
    )
    run.add_argument("--timeout-secs", default="1800")
    run.add_argument("--materialized-corpus", action="store_true")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.command == "check":
        return cmd_check(args)
    return cmd_run(args)


if __name__ == "__main__":
    raise SystemExit(main())
