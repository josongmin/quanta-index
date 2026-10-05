"""Replay query observation on/off or separate scanner A/B captures.

Run identical frozen runner inputs with --query-stage-observation enabled/disabled, a fresh
state root for each, and the same query protocol. This replay refuses differing
results, model/input identities or repetition coverage. Values describe these
samples only; host quietness and interleaved repetitions remain separate gates.
The control toggles plane stage/trace collection. Backend clock reads execute
in both arms, so this is not an instrumentation-free backend comparison.
The scanner A/B mode requires separate local source/build custody receipts,
including a distinct source identity and searchd binary for each arm. It
compares normalized outputs and work counts but does not qualify speed.
"""

from __future__ import annotations

import argparse
import statistics
from pathlib import Path

from tools.benchmark.retrieval import run as pairrun
from tools.benchmark.retrieval.conditional_proof import canonical, load, sha
from tools.benchmark.retrieval.finite_json import is_finite_json_number
from tools.benchmark.retrieval.scanner_build_custody import verify as verify_scanner_build
from tools.benchmark.retrieval.scanner_source_identity import verify_pair as verify_scanner_source_pair


def _without_code_search_work_clocks(planner_trace: list, *, allow_clocks: bool = True) -> list:
    """Remove validated observation clocks, preserving every non-clock fact."""
    parent_prefixes = (
        "code_search.execution.candidate_ns=",
        "code_search.execution.sort_page_ns=",
        "code_search.execution.preview_ns=",
    )
    child_prefixes = (
        "code_search.execution.typo_shortlist_admission_ns=",
        "code_search.execution.typo_source_token_scan_ns=",
        "code_search.execution.typo_materialize_ns=",
    )
    clock_prefixes = parent_prefixes + child_prefixes
    result = []
    seen: set[str] = set()
    clocks: dict[str, int] = {}
    for entry in planner_trace:
        detail = entry.get("detail") if isinstance(entry, dict) else None
        prefix = next(
            (
                candidate
                for candidate in clock_prefixes
                if isinstance(detail, str) and detail.startswith(candidate)
            ),
            None,
        )
        if prefix is None:
            result.append(entry)
            continue
        value = detail[len(prefix) :]
        if not allow_clocks:
            raise ValueError("disabled query observation emitted a CodeSearch work clock")
        if (
            prefix in seen
            or not value.isascii()
            or not value.isdecimal()
            or len(value) > 20
            or int(value) > (1 << 64) - 1
        ):
            raise ValueError("on/off CodeSearch work clock is malformed or duplicated")
        seen.add(prefix)
        clocks[prefix] = int(value)
    modes = [
        entry["detail"]
        for entry in planner_trace
        if isinstance(entry, dict)
        and isinstance(entry.get("detail"), str)
        and entry["detail"].startswith("code_search.execution.mode=")
    ]
    if allow_clocks and modes:
        expected = set(parent_prefixes)
        if modes == ["code_search.execution.mode=typo_fallback"] or modes == [
            "code_search.execution.mode=typo_explicit"
        ]:
            expected.update(child_prefixes)
        elif modes not in (
            ["code_search.execution.mode=ordinary"],
            ["code_search.execution.mode=components"],
        ):
            raise ValueError("on/off CodeSearch work clock hierarchy has an unknown execution mode")
        if seen != expected or (
            expected.intersection(child_prefixes)
            and sum(clocks[key] for key in child_prefixes) > clocks[parent_prefixes[0]]
        ):
            raise ValueError("on/off CodeSearch work clock hierarchy is invalid")
    elif seen:
        raise ValueError("on/off CodeSearch work clock lacks execution mode")
    return result


def compare(
    on: dict,
    off: dict,
    on_phases: dict,
    off_phases: dict,
    on_diagnostic: dict,
    off_diagnostic: dict,
    pack: dict,
) -> dict:
    for record, phases, diagnostic, policy in (
        (on, on_phases, on_diagnostic, "enabled"),
        (off, off_phases, off_diagnostic, "disabled"),
    ):
        if (
            record.get("schema_version") != 5
            or record.get("span_accounting_version") != 1
            or diagnostic.get("schema_version") not in (5, 6, 7, 8, 9)
            or phases.get("system") != "quanta"
            or not phases.get("query_protocol")
        ):
            raise ValueError("overhead comparison requires v5 records and explicit on/off protocol")
        if diagnostic.get("server_observation") != pairrun.server_observation_configuration(policy):
            raise ValueError("overhead capture lacks its actual server observation policy")
        pairrun._validate_phase_metrics(phases, "overhead phase metrics")
        pairrun.validate_retrieval_diagnostic(diagnostic, record, phases["record_sha256"], pack)
    if on_diagnostic["schema_version"] != off_diagnostic["schema_version"] or (
        on_diagnostic["schema_version"] in (6, 7, 8, 9)
        and on_diagnostic["hybrid_fetch_policy"] != off_diagnostic["hybrid_fetch_policy"]
    ):
        raise ValueError("on/off hybrid fetch policy or diagnostic version differs")
    for key in (
        "strategy",
        "query_schedule",
        "query_protocol",
        "task_count",
        "route_count",
        "measurement_repetitions",
        "warmup_passes",
        "runner_binary_sha256",
        "file_count",
        "chunk_count",
        "symbol_count",
        "symbol_coverage",
        "symbol_unsupported_details",
        "symbol_coverage_policy",
        "symbol_grammars",
        "symbol_producer_identity",
        "symbol_producer_policy_sha256",
        "symbol_preflight_sha256",
        "symbol_incomplete_files",
        "empty_scopes",
        "symbol_only_scopes",
    ):
        if key not in on_phases or on_phases[key] != off_phases.get(key):
            raise ValueError(f"on/off frozen configuration differs: {key}")

    def identities(record):
        return sorted(
            (
                capture["chunk_strategy"],
                str(capture["chunk_config"]),
                capture["model"],
                capture["model_revision"],
                capture["execution_profile_sha256"],
                capture["runner_binary"]["digest"],
                capture["searchd_binary"]["binary_digest"],
            )
            for capture in record["captures"].values()
        )

    if (
        not on.get("captures")
        or identities(on) != identities(off)
        or on.get("query_pack_sha256") != off.get("query_pack_sha256")
        or not on.get("query_pack_sha256")
    ):
        raise ValueError("on/off model/daemon/input identity differs")

    def answers(record):
        return [
            (row["route"], row["task_id"], row["status"], row["error"], row["candidates"])
            for row in record["results"]
        ]

    if (
        not on.get("results")
        or answers(on) != answers(off)
        or any(row["status"] not in ("success", "capped", "abstained") for row in on["results"])
    ):
        raise ValueError("on/off answer or ranking differs, or execution failed")

    def observable_rows(record, diagnostic, *, allow_work_clocks):
        expected_order = [(row["route"], row["task_id"]) for row in record["results"]]
        rows = diagnostic["results"]
        if [(row["route"], row["task_id"]) for row in rows] != expected_order:
            raise ValueError("on/off diagnostic row order differs from the executed record")
        projected = []
        for row in rows:
            response = row["response"]
            if isinstance(response, dict):
                # Fresh daemon runs assign different transport IDs. Only stage
                # clocks change under this policy; page, planner and lane facts do not.
                explanation = response["explanation"]
                if isinstance(explanation, dict):
                    # CodeSearch work clocks are response observations under
                    # the enabled policy. Keep every work count, mode, scope,
                    # and other planner entry in the equality check.
                    planner_trace = explanation.get("planner_trace")
                    if isinstance(planner_trace, list):
                        planner_trace = _without_code_search_work_clocks(
                            planner_trace, allow_clocks=allow_work_clocks
                        )
                    response = {
                        **response,
                        "explanation": {
                            key: planner_trace if key == "planner_trace" else value
                            for key, value in explanation.items()
                            if key not in ("request_id", "stage_timings")
                        },
                    }
            projected.append({**row, "response": response})
        return projected

    if observable_rows(on, on_diagnostic, allow_work_clocks=True) != observable_rows(
        off, off_diagnostic, allow_work_clocks=False
    ):
        raise ValueError("on/off observable diagnostic response or page differs")
    summaries = []
    for route, tasks in on_phases["warm_latencies_ms"].items():
        if route not in off_phases["warm_latencies_ms"]:
            raise ValueError("off capture lacks measured route")
        for task_id, measured_on in tasks.items():
            measured_off = off_phases["warm_latencies_ms"][route].get(task_id)
            repetitions = on_phases["measurement_repetitions"]
            if (
                not isinstance(measured_off, list)
                or len(measured_on) != repetitions
                or len(measured_off) != repetitions
                or repetitions < 2
                or any(
                    not is_finite_json_number(value) or value <= 0
                    for value in measured_on + measured_off
                )
            ):
                raise ValueError("on/off sample coverage is missing or invalid")
            left, right = statistics.median(measured_on), statistics.median(measured_off)
            summaries.append(
                {
                    "route": route,
                    "task_id": task_id,
                    "samples_each": repetitions,
                    "on_median_ms": left,
                    "off_median_ms": right,
                    "delta_ms": left - right,
                    "relative_delta": left / right - 1,
                }
            )
    on_keys = {(row["route"], row["task_id"]) for row in summaries}
    off_keys = {
        (route, task) for route, tasks in off_phases["warm_latencies_ms"].items() for task in tasks
    }
    expected = {(row["route"], row["task_id"]) for row in on["results"]}
    if len(on_keys) != len(summaries) or on_keys != off_keys or on_keys != expected:
        raise ValueError("on/off task inventory differs from actual executed rows")
    return {
        "schema_version": 2,
        "status": "diagnostic_unqualified",
        "scorer_identity": "query-stage-clock-overhead-v2",
        "measurement_scope": "plane_stage_and_response_trace_observation",
        "backend_clock_reads": "enabled_in_both_arms",
        "qualification_limits": [
            "not_total_instrumentation_overhead",
            "not_ipc_attribution",
            "host_and_repetition_qualification_not_established",
        ],
        "rows": summaries,
    }


def _same_json(left: object, right: object) -> bool:
    """Compare JSON values without Python's bool/int or int/float equality aliases."""
    return canonical(left) == canonical(right)


def _scanner_identity(record: dict, phases: dict, declared: dict) -> dict:
    if set(declared) != {
        "source_identity_sha256",
        "runner_binary_sha256",
        "searchd_binary_sha256",
    }:
        raise ValueError("scanner A/B requires explicit source and binary identities")
    revision = declared["source_identity_sha256"]
    if (
        not isinstance(revision, str)
        or len(revision) != 64
        or any(char not in "0123456789abcdef" for char in revision)
    ):
        raise ValueError("scanner A/B source identity must be a lowercase SHA-256")
    for key in ("runner_binary_sha256", "searchd_binary_sha256"):
        value = declared[key]
        if (
            not isinstance(value, str)
            or len(value) != 64
            or any(char not in "0123456789abcdef" for char in value)
        ):
            raise ValueError(f"scanner A/B {key} must be a lowercase SHA-256")
    if phases.get("runner_binary_sha256") != declared["runner_binary_sha256"]:
        raise ValueError("scanner A/B phase runner binary differs from declared identity")
    captures = record.get("captures")
    if not isinstance(captures, dict) or not captures:
        raise ValueError("scanner A/B requires captured binary and model identities")
    projected = {}
    for capture_id, capture in captures.items():
        if (
            not isinstance(capture, dict)
            or not isinstance(capture.get("runner_binary"), dict)
            or not isinstance(capture.get("searchd_binary"), dict)
            or capture["runner_binary"].get("digest") != declared["runner_binary_sha256"]
            or capture["searchd_binary"].get("binary_digest") != declared["searchd_binary_sha256"]
        ):
            raise ValueError("scanner A/B captured binary differs from declared identity")
        projected[capture_id] = {
            key: capture[key]
            for key in (
                "system",
                "chunk_strategy",
                "chunk_config",
                "model",
                "model_revision",
                "execution_profile",
                "execution_profile_sha256",
            )
        }
    provenance = record.get("route_provenance")
    if (
        not isinstance(provenance, dict)
        or not provenance
        or any(
            not isinstance(owner, dict) or owner.get("capture_id") not in projected
            for owner in provenance.values()
        )
    ):
        raise ValueError("scanner A/B route capture identity is missing")
    return {
        "captures": sorted(projected.values(), key=canonical),
        "routes": {route: projected[owner["capture_id"]] for route, owner in provenance.items()},
    }


def _scanner_response_rows(record: dict, diagnostic: dict, *, allow_clocks: bool) -> list:
    rows = diagnostic["results"]
    expected_order = [(row["route"], row["task_id"]) for row in record["results"]]
    if [(row["route"], row["task_id"]) for row in rows] != expected_order:
        raise ValueError("scanner A/B diagnostic row order differs from executed record")
    projected = []
    for row in rows:
        response = row["response"]
        if isinstance(response, dict) and isinstance(response.get("explanation"), dict):
            explanation = response["explanation"]
            trace = explanation.get("planner_trace")
            if isinstance(trace, list):
                trace = _without_code_search_work_clocks(trace, allow_clocks=allow_clocks)
            stage_timings = explanation.get("stage_timings")
            if isinstance(stage_timings, list):
                stage_timings = [
                    {key: value for key, value in timing.items() if key != "elapsed_ns"}
                    for timing in stage_timings
                ]
            response = {
                **response,
                "explanation": {
                    key: (
                        trace
                        if key == "planner_trace"
                        else stage_timings
                        if key == "stage_timings"
                        else value
                    )
                    for key, value in explanation.items()
                    # The canonical diagnostic validator checks these transport-local
                    # IDs and clocks within each capture before this projection.
                    # Stage order, calls and returned candidate counts are work,
                    # not timing, and must remain equal across scanner arms.
                    if key != "request_id"
                },
            }
        projected.append({**row, "response": response})
    return projected


def _scanner_query_timing(timing: dict) -> dict:
    # These fields are the complete clock inventory in the canonical completed
    # response timing schema. Keep status, output size/digest and schedule.
    clocks = {
        "start_ns",
        "end_ns",
        "sdk_execute_ns",
        "sdk_post_execute_ns",
        "runner_result_materialize_ns",
    }
    return {
        **timing,
        "observations": [
            {key: value for key, value in row.items() if key not in clocks}
            for row in timing["observations"]
        ],
    }


def _scanner_ingest_work(ingest: dict) -> dict:
    """Retain validated ingest work counts without comparing local ACKs or clocks."""
    receipt = ingest["receipt"]
    observation = ingest["observation"]
    semantic = observation["semantic"]
    receipt_work = {
        key: value
        for key, value in receipt.items()
        if key not in {"generation", "manifest_digest", "batch_digest", "durable_sequence"}
    }
    observation_work = {
        key: value
        for key, value in observation.items()
        if key
        not in {
            "request_id",
            "generation",
            "batch_digest",
            "activation_ns",
            "finalize_ns",
            "lexical_build_ns",
            "lexical_stages",
            "semantic",
        }
    }
    observation_work["semantic"] = {
        key: value for key, value in semantic.items() if key != "durations"
    }
    return {"receipt": receipt_work, "observation": observation_work}


def compare_scanner(
    baseline: dict,
    candidate: dict,
    baseline_phases: dict,
    candidate_phases: dict,
    baseline_diagnostic: dict,
    candidate_diagnostic: dict,
    pack: dict,
    baseline_identity: dict,
    candidate_identity: dict,
) -> dict:
    """Compare separate scanner builds; this is parity and sample diagnostics, not speed proof."""
    for record, phases, diagnostic in (
        (baseline, baseline_phases, baseline_diagnostic),
        (candidate, candidate_phases, candidate_diagnostic),
    ):
        if (
            record.get("schema_version") != 5
            or record.get("span_accounting_version") != 1
            or diagnostic.get("schema_version") != 9
            or phases.get("system") != "quanta"
            or phases.get("schema_version") != 4
            or not phases.get("query_protocol")
        ):
            raise ValueError("scanner A/B requires current v5/v9/v4 capture and query protocol")
        pairrun._validate_phase_metrics(phases, "scanner A/B phase metrics")
        pairrun.validate_completed_query_timing(phases, record, require_output_validation=True)
        pairrun.validate_retrieval_diagnostic(diagnostic, record, phases["record_sha256"], pack)
    baseline_captures = _scanner_identity(baseline, baseline_phases, baseline_identity)
    candidate_captures = _scanner_identity(candidate, candidate_phases, candidate_identity)
    if baseline_identity["source_identity_sha256"] == candidate_identity["source_identity_sha256"] or (
        baseline_identity["searchd_binary_sha256"] == candidate_identity["searchd_binary_sha256"]
    ):
        raise ValueError("scanner A/B requires distinct attested source and searchd binary")
    if not _same_json(baseline_captures, candidate_captures):
        raise ValueError("scanner A/B captured corpus/model/profile configuration differs")
    if not _same_json(baseline.get("comparison_contract"), candidate.get("comparison_contract")):
        raise ValueError("scanner A/B output comparison contract differs")
    for record in (baseline, candidate):
        if not isinstance(record.get("runner"), dict) or not record["runner"].get("run_id"):
            raise ValueError("scanner A/B runner protocol is missing")
    if not _same_json(
        {key: value for key, value in baseline["runner"].items() if key != "run_id"},
        {key: value for key, value in candidate["runner"].items() if key != "run_id"},
    ):
        raise ValueError("scanner A/B runner protocol differs")
    if baseline.get("query_pack_sha256") != candidate.get("query_pack_sha256") or not baseline.get(
        "query_pack_sha256"
    ):
        raise ValueError("scanner A/B query pack differs")
    # Only per-run record digest, runner binary and measured clocks may differ.
    phase_exclusions = {
        "record_sha256",
        "runner_binary_sha256",
        "phases_ms",
        "total_ms",
        "warm_latencies_ms",
        "cold_latencies_ms",
    }
    baseline_config = {k: v for k, v in baseline_phases.items() if k not in phase_exclusions}
    candidate_config = {k: v for k, v in candidate_phases.items() if k not in phase_exclusions}
    if "query_timing" in baseline_config:
        baseline_config["query_timing"] = _scanner_query_timing(baseline_config["query_timing"])
    if "query_timing" in candidate_config:
        candidate_config["query_timing"] = _scanner_query_timing(candidate_config["query_timing"])
    if not _same_json(baseline_config, candidate_config):
        raise ValueError("scanner A/B frozen phase configuration or work counts differ")
    diagnostic_exclusions = {"record_sha256", "results", "runner_timing_detail_ms", "ingest"}
    baseline_config = {
        k: v for k, v in baseline_diagnostic.items() if k not in diagnostic_exclusions
    }
    candidate_config = {
        k: v for k, v in candidate_diagnostic.items() if k not in diagnostic_exclusions
    }
    if not _same_json(baseline_config, candidate_config):
        raise ValueError("scanner A/B diagnostic query/protocol/observation configuration differs")
    if not _same_json(
        _scanner_ingest_work(baseline_diagnostic["ingest"]),
        _scanner_ingest_work(candidate_diagnostic["ingest"]),
    ):
        raise ValueError("scanner A/B ingest work counts or source scope differ")

    def answers(record):
        projected = []
        for row in record["results"]:
            if not isinstance(row.get("timings"), dict) or set(row["timings"]) != {
                "query_latency_ms"
            }:
                raise ValueError("scanner A/B record timing shape is unsupported")
            projected.append({key: value for key, value in row.items() if key != "timings"})
        return projected

    if (
        not baseline.get("results")
        or not _same_json(answers(baseline), answers(candidate))
        or any(
            row["status"] not in ("success", "capped", "abstained") for row in baseline["results"]
        )
    ):
        raise ValueError("scanner A/B status, path, order, score or answer differs")
    observation = baseline_diagnostic["server_observation"]
    enabled = pairrun.server_observation_configuration("enabled")
    disabled = pairrun.server_observation_configuration("disabled")
    if not _same_json(observation, enabled) and not _same_json(observation, disabled):
        raise ValueError("scanner A/B has unsupported observation policy")
    if not _same_json(
        _scanner_response_rows(
            baseline, baseline_diagnostic, allow_clocks=_same_json(observation, enabled)
        ),
        _scanner_response_rows(
            candidate, candidate_diagnostic, allow_clocks=_same_json(observation, enabled)
        ),
    ):
        raise ValueError("scanner A/B observable response, cursor or work counter differs")
    keys = {(row["route"], row["task_id"]) for row in baseline["results"]}
    if (
        len(keys) != len(baseline["results"])
        or keys
        != {
            (route, task)
            for route, tasks in baseline_phases["warm_latencies_ms"].items()
            for task in tasks
        }
        or keys
        != {
            (route, task)
            for route, tasks in candidate_phases["warm_latencies_ms"].items()
            for task in tasks
        }
    ):
        raise ValueError("scanner A/B measured task coverage differs")
    summaries = []
    repetitions = baseline_phases["measurement_repetitions"]
    for route, task_id in sorted(keys):
        left = baseline_phases["warm_latencies_ms"][route][task_id]
        right = candidate_phases["warm_latencies_ms"][route][task_id]
        if (
            repetitions < 2
            or len(left) != repetitions
            or len(right) != repetitions
            or any(not is_finite_json_number(value) or value <= 0 for value in left + right)
        ):
            raise ValueError("scanner A/B sample coverage is missing or invalid")
        baseline_median = statistics.median(left)
        candidate_median = statistics.median(right)
        summaries.append(
            {
                "route": route,
                "task_id": task_id,
                "samples_each": repetitions,
                "baseline_median_ms": baseline_median,
                "candidate_median_ms": candidate_median,
                "delta_ms": candidate_median - baseline_median,
                "relative_delta": candidate_median / baseline_median - 1,
            }
        )
    return {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "scorer_identity": "scanner-ab-parity-v1",
        "identity_scope": "comparator_supplied_source_binary_claims_only",
        "baseline_identity": baseline_identity,
        "candidate_identity": candidate_identity,
        "allowed_work_counter_differences": [],
        "qualification_limits": [
            "source_build_custody_not_verified_by_comparator_function",
            "host_and_repetition_qualification_not_established",
        ],
        "rows": summaries,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scanner-ab", action="store_true")
    for name in (
        "on-record",
        "off-record",
        "on-phases",
        "off-phases",
        "on-diagnostic",
        "off-diagnostic",
        "baseline-record",
        "candidate-record",
        "baseline-phases",
        "candidate-phases",
        "baseline-diagnostic",
        "candidate-diagnostic",
    ):
        parser.add_argument(f"--{name}", type=Path)
    for name in ("baseline", "candidate"):
        parser.add_argument(f"--{name}-custody", type=Path)
    parser.add_argument("--pack", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        overhead_paths = [
            args.on_record,
            args.off_record,
            args.on_phases,
            args.off_phases,
            args.on_diagnostic,
            args.off_diagnostic,
        ]
        scanner_paths = [
            args.baseline_record,
            args.candidate_record,
            args.baseline_phases,
            args.candidate_phases,
            args.baseline_diagnostic,
            args.candidate_diagnostic,
        ]
        scanner_claims = [args.baseline_custody, args.candidate_custody]
        if args.scanner_ab:
            if any(path is not None for path in overhead_paths) or any(
                value is None for value in [*scanner_paths, *scanner_claims]
            ):
                raise ValueError("scanner A/B requires only complete baseline/candidate inputs")
            paths = [*scanner_paths, args.pack]
            if any(path.is_symlink() or not path.is_file() for path in [*paths, *scanner_claims]):
                raise ValueError("scanner A/B replay refuses missing or symlinked inputs")
        else:
            if (
                any(path is not None for path in scanner_paths)
                or any(value is not None for value in scanner_claims)
                or any(path is None for path in overhead_paths)
            ):
                raise ValueError("on/off comparison requires only complete on/off inputs")
            paths = [*overhead_paths, args.pack]
        payloads = [path.read_bytes() for path in paths]
        phases = [load(payload) for payload in payloads[2:4]]
        if any(
            phase["record_sha256"] != sha(raw)
            for phase, raw in zip(phases, payloads[:2], strict=True)
        ):
            raise ValueError("on/off phase record digest mismatch")
        pack = load(payloads[-1])
        corpus = {"repository_commit": pack["repository_commit"], "files": pack["file_universe"]}
        for phase, path in zip(phases, paths[2:4], strict=True):
            pairrun.symbol_coverage.verify_artifact(phase, path, corpus)
        objects = [load(payload) for payload in payloads]
        custody_payloads = []
        if args.scanner_ab:
            custody_payloads = [path.read_bytes() for path in scanner_claims]
            receipts = [load(raw) for raw in custody_payloads]
            for arm, receipt in enumerate(receipts):
                verify_scanner_build(receipt)
                captured_pack = receipt["capture_outputs"]["pack"]
                if (
                    (arm == 0 and Path(captured_pack["path"]).resolve() != args.pack.resolve())
                    or captured_pack["sha256"] != sha(args.pack.read_bytes())
                ):
                    raise ValueError("scanner custody projected pack differs from replay input")
                for role, index in (("record", arm), ("phases", arm + 2), ("diagnostic", arm + 4)):
                    captured = receipt["capture_outputs"][role]
                    if Path(captured["path"]).resolve() != scanner_paths[index].resolve() or captured["sha256"] != sha(payloads[index]):
                        raise ValueError(f"scanner custody {role} output differs from replay input")
            verify_scanner_source_pair(receipts[0]["source_identity"], receipts[1]["source_identity"])
            if set(receipts[0]["inputs"]) != set(receipts[1]["inputs"]) or any(
                receipts[0]["inputs"][role]["files"] != receipts[1]["inputs"][role]["files"]
                for role in receipts[0]["inputs"]
            ):
                raise ValueError("scanner A/B corpus, query, suite or template bytes differ")
            effective_env = [
                {key: value for key, value in receipt["execution_env_sha256"].items()
                 if key not in {"CARGO_TARGET_DIR", "PYTHONPATH"}}
                for receipt in receipts
            ]
            if effective_env[0] != effective_env[1]:
                raise ValueError("scanner A/B relevant build/capture environment differs")
            tool_fingerprints = [
                {key: (value["sha256"], value.get("version"))
                 for key, value in receipt["tools"].items()}
                for receipt in receipts
            ]
            if tool_fingerprints[0] != tool_fingerprints[1]:
                raise ValueError("scanner A/B build/capture tools differ")
            identities = [
                {
                    "source_identity_sha256": receipt["source_identity"]["identity_sha256"],
                    "runner_binary_sha256": receipt["binaries"]["runner"]["sha256"],
                    "searchd_binary_sha256": receipt["binaries"]["searchd"]["sha256"],
                }
                for receipt in receipts
            ]
            baseline_identity, candidate_identity = identities
            result = compare_scanner(*objects, baseline_identity, candidate_identity)
            result["identity_scope"] = "local_scanner_source_build_receipt_v1"
            result["qualification_limits"] = [
                "local_build_custody_not_remote_attestation",
                "host_and_repetition_qualification_not_established",
            ]
        else:
            result = compare(*objects)

        with args.out.open("xb") as stream:
            stream.write(
                canonical({**result, "input_sha256": [sha(raw) for raw in payloads + custody_payloads]}) + b"\n"
            )
    except (ValueError, OSError, KeyError, TypeError, pairrun.RunError) as error:
        mode = "scanner A/B" if args.scanner_ab else "overhead"
        parser.exit(2, f"{mode} replay refused: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
