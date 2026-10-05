"""Scanner A/B parity on complete, independently validated retrieval captures."""

from __future__ import annotations

import copy
import json

import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import query_timing_overhead as scanner
from tools.benchmark.retrieval import retrieval_contract as rc
from tools.benchmark.retrieval import run as pairrun
from tools.benchmark.retrieval import semble as semble_adapter
from tools.ci.tests.test_retrieval_benchmark import _pair_stage


def _arm(record: dict, phase: dict, diagnostic: dict, *, source: str, searchd: str):
    record = copy.deepcopy(record)
    phase = copy.deepcopy(phase)
    diagnostic = copy.deepcopy(diagnostic)
    record["span_accounting_version"] = 1
    for capture in record["captures"].values():
        capture["searchd_binary"]["binary_digest"] = searchd
    record_sha = ev.digest(ev.canonical(record))
    phase["record_sha256"] = record_sha
    diagnostic["record_sha256"] = record_sha
    phase["schema_version"] = 4
    phase["measurement_repetitions"] = 2
    phase["query_protocol"] = pairrun.build_query_protocol(phase["query_schedule"], 0, 1, 2)
    phase["warm_latencies_ms"] = {
        route: {task: [1.5, 1.5] for task in tasks}
        for route, tasks in phase["warm_latencies_ms"].items()
    }
    phase["phases_ms"]["warm_query"] = 1.5 * len(record["results"]) * 2
    phase["total_ms"] = sum(
        value
        for key, value in phase["phases_ms"].items()
        if key not in ("sdk_publish", "sdk_activate")
    )
    record_rows = {(row["task_id"], row["route"]): row for row in record["results"]}
    observations = []
    cursor_ns = 1_000_000
    protocol = phase["query_protocol"]
    schedule = [("cold", 0, protocol["cold_probe_task_id"])]
    schedule.extend(
        ("warmup", iteration, task)
        for iteration, tasks in enumerate(protocol["warmup_schedules"])
        for task in tasks
    )
    schedule.extend(
        ("measured", iteration, task)
        for iteration, tasks in enumerate(protocol["measurement_schedules"])
        for task in tasks
    )
    route = next(iter(phase["warm_latencies_ms"]))
    for kind, iteration, task in schedule:
        row = record_rows[(task, route)]
        duration_ns = 1_500_000 if kind == "measured" else 1_000_000
        observations.append(
            {
                "task_id": task,
                "route": route,
                "phase": kind,
                "iteration": iteration,
                "start_ns": cursor_ns,
                "end_ns": cursor_ns + duration_ns,
                "sdk_execute_ns": 100,
                "sdk_post_execute_ns": 10,
                "runner_result_materialize_ns": 10,
                "status": row["status"],
                "output_bytes": len(ev.canonical(row)),
                "output_sha256": rc.completed_output_sha256(row),
            }
        )
        cursor_ns += duration_ns
    phase["cold_latencies_ms"] = {route: 1.0}
    phase["query_timing"] = {
        "boundary": semble_adapter.QUERY_TIMING_BOUNDARY,
        "clock": semble_adapter.QUERY_TIMING_CLOCK,
        "output_validation": rc.COMPLETED_OUTPUT_VALIDATION,
        "observations": observations,
    }
    identity = {
        "source_identity_sha256": source,
        "runner_binary_sha256": phase["runner_binary_sha256"],
        "searchd_binary_sha256": searchd,
    }
    return record, phase, diagnostic, identity


def _pair(tmp_path):
    stage = _pair_stage(tmp_path, diagnostic_version=9)
    path = next(stage["stage"].glob("rep-00/quanta/strategy-*/retrieval-diagnostic.json"))
    suite = json.loads(stage["suite_path"].read_text())
    pack = json.loads((stage["stage"] / "query-pack.json").read_text())
    pack, _ = pairrun.project_pack_and_suite(pack, suite, ["lexical"])
    record = json.loads(path.with_name("record.json").read_text())
    phase = json.loads(path.with_name("phase-metrics.json").read_text())
    diagnostic = json.loads(path.read_text())
    baseline = _arm(record, phase, diagnostic, source="a" * 64, searchd="b" * 64)
    candidate = _arm(record, phase, diagnostic, source="c" * 64, searchd="d" * 64)
    return baseline, candidate, pack


def _compare(baseline, candidate, pack):
    return scanner.compare_scanner(
        baseline[0],
        candidate[0],
        baseline[1],
        candidate[1],
        baseline[2],
        candidate[2],
        pack,
        baseline[3],
        candidate[3],
    )


def test_scanner_ab_preserves_parity_and_declared_binary_difference(tmp_path):
    baseline, candidate, pack = _pair(tmp_path)
    result = _compare(baseline, candidate, pack)
    assert result["status"] == "diagnostic_unqualified"
    assert result["scorer_identity"] == "scanner-ab-parity-v1"
    assert result["identity_scope"] == "comparator_supplied_source_binary_claims_only"
    assert result["allowed_work_counter_differences"] == []
    assert result["baseline_identity"] != result["candidate_identity"]
    assert len(result["rows"]) == len(baseline[0]["results"])
    assert all(row["delta_ms"] == 0 for row in result["rows"])
    # Validated clocks can vary; output digests, work counts and page facts cannot.
    for observation in candidate[1]["query_timing"]["observations"]:
        observation["start_ns"] += 1
        observation["end_ns"] += 1
    for row in candidate[2]["results"]:
        row["response"]["explanation"]["request_id"] += 100
    candidate[2]["runner_timing_detail_ms"]["daemon_shutdown"] += 1
    assert _compare(baseline, candidate, pack)["rows"]


@pytest.mark.parametrize(
    "mutation",
    [
        "status",
        "order",
        "score",
        "cursor",
        "corpus",
        "model",
        "binary",
        "source",
        "work_counter",
        "stage_work_counter",
        "ingest_counter",
        "comparison_contract",
        "runner_protocol",
        "output_digest",
        "type_alias",
    ],
)
def test_scanner_ab_refuses_independent_semantic_or_custody_delta(tmp_path, mutation):
    baseline, candidate, pack = _pair(tmp_path)
    record, phase, diagnostic, identity = candidate
    if mutation == "status":
        record["results"][0]["status"] = "timeout"
    elif mutation == "order":
        record["results"].reverse()
        diagnostic["results"].reverse()
    elif mutation == "score":
        row = next(row for row in diagnostic["results"] if row["candidates"])
        row["candidates"][0]["score"] = -0.0
    elif mutation == "cursor":
        diagnostic["results"][0]["response"]["window"]["outcome"]["kind"] = "lower_bound"
    elif mutation == "corpus":
        next(iter(record["captures"].values()))["chunk_config"] = {"changed": True}
    elif mutation == "model":
        next(iter(record["captures"].values()))["model"] = "different-model"
    elif mutation == "binary":
        identity["searchd_binary_sha256"] = "e" * 64
    elif mutation == "source":
        identity["source_identity_sha256"] = baseline[3]["source_identity_sha256"]
    elif mutation == "work_counter":
        diagnostic["results"][0]["response"]["explanation"]["planner_trace"].append(
            {"stage": "merge", "detail": "code_search.execution.posting_probes=999"}
        )
    elif mutation == "stage_work_counter":
        row = next(
            row
            for row in diagnostic["results"]
            if any(
                stage["stage"] == "lexical.search"
                for stage in row["response"]["explanation"]["stage_timings"]
            )
        )
        stage = next(
            stage
            for stage in row["response"]["explanation"]["stage_timings"]
            if stage["stage"] == "lexical.search"
        )
        stage["returned_candidates"] += 1
    elif mutation == "ingest_counter":
        diagnostic["ingest"]["receipt"]["accepted_replace_scopes"] += 1
    elif mutation == "comparison_contract":
        record["comparison_contract"]["span_unit"] = "changed-span-unit"
    elif mutation == "runner_protocol":
        record["runner"]["tokenizer_budget_version"] = "different-budget"
    elif mutation == "output_digest":
        phase["query_timing"]["observations"][0]["output_sha256"] = "f" * 64
    elif mutation == "type_alias":
        diagnostic["results"][0]["response"]["window"]["coverage"]["lanes"][0]["contributed"] = 1
    with pytest.raises((ValueError, pairrun.RunError)):
        _compare(baseline, candidate, pack)
