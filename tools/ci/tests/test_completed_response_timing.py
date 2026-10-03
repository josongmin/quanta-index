"""Continuous completed-response clocks, including parent normalization."""

import copy
import json
import os
import sys
import time

import pytest

from tools.benchmark.retrieval import run as pairrun
from tools.benchmark.retrieval import semble
from tools.ci.tests import test_retrieval_benchmark as fixtures


def _timing(phase, record):
    protocol = phase["query_protocol"]
    rows = {(row["task_id"], row["route"]): row for row in record["results"]}
    routes = sorted(phase["warm_latencies_ms"])
    schedule = [("cold", 0, protocol["cold_probe_task_id"])]
    for label, schedules in (
        ("warmup", protocol["warmup_schedules"]),
        ("measured", protocol["measurement_schedules"]),
    ):
        schedule.extend(
            (label, iteration, task) for iteration, tasks in enumerate(schedules) for task in tasks
        )
    observations = []
    previous_end = 0
    for label, iteration, task in schedule:
        for route in routes:
            duration_ms = (
                phase["cold_latencies_ms"][route]
                if label == "cold"
                else phase["warm_latencies_ms"][route][task][iteration]
                if label == "measured"
                else 0.000001
            )
            row = dict(rows[(task, route)])
            row.pop("timings")
            duration_ns = round(duration_ms * 1e6)
            observations.append(
                {
                    "task_id": task,
                    "route": route,
                    "phase": label,
                    "iteration": iteration,
                    "start_ns": previous_end,
                    "end_ns": previous_end + duration_ns,
                    "status": row["status"],
                    "output_bytes": len(pairrun.canonical(row)),
                }
            )
            previous_end += duration_ns
    return {
        "boundary": semble.QUERY_TIMING_BOUNDARY,
        "clock": semble.QUERY_TIMING_CLOCK,
        "observations": observations,
    }


def _add_timing(stage, *, sdk_children=False):
    for layout in stage["rep_layouts"]:
        for strategy, record_path in layout["quanta"].items():
            path = fixtures.Path(layout["quanta_phase_metrics"][strategy])
            phase = pairrun.read_json(path)
            phase["query_timing"] = _timing(phase, pairrun.read_json(fixtures.Path(record_path)))
            if sdk_children:
                phase["schema_version"] = 4
                for observation in phase["query_timing"]["observations"]:
                    duration = observation["end_ns"] - observation["start_ns"]
                    observation.update(
                        sdk_execute_ns=duration // 2,
                        sdk_post_execute_ns=duration // 4,
                        runner_result_materialize_ns=duration // 8,
                    )
            path.write_text(json.dumps(phase))
        native_path = fixtures.Path(layout["semble"]).parent / "native.json"
        native = pairrun.read_json(native_path)
        path = fixtures.Path(layout["semble_phase_metrics"])
        phase = pairrun.read_json(path)
        phase["query_timing"] = _timing(phase, pairrun.read_json(fixtures.Path(layout["semble"])))
        native["query_timing"] = phase["query_timing"]
        route = next(iter(phase["warm_latencies_ms"]))
        native["cold_latency_ms"] = phase["cold_latencies_ms"][route]
        native["latencies_ms"] = phase["warm_latencies_ms"][route]
        path.write_text(json.dumps(phase))
        native_path.write_text(json.dumps(native))
        manifest_path = fixtures.Path(layout["quanta_manifest"])
        manifest = pairrun.read_json(manifest_path)
        for entry in manifest["runs"]:
            entry["phase_metrics_digest"] = pairrun.sha_file(
                manifest_path.parent / entry["phase_metrics"]
            )
        manifest_path.write_text(json.dumps(manifest))
    run_manifest = pairrun.read_json(stage["manifest_path"])
    run_manifest["artifacts"]["phase_metrics_digests"] = {
        ref: pairrun.sha_file(stage["stage"] / ref)
        for ref in run_manifest["artifacts"]["phase_metrics"]
    }
    stage["manifest_path"].write_text(json.dumps(run_manifest))


def test_paired_completed_response_clocks_require_binary_source_attestation(tmp_path):
    stage = fixtures._pair_stage(
        tmp_path,
        repetitions=5,
        qualified_speed_sample=True,
        scope="qualified",
        claims={"speed": True},
    )
    _add_timing(stage)
    verdict = fixtures._stage_verdict(stage)
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "binary_build_source_unattested"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["proof_digest"] is None


def test_sdk_timing_children_preserve_outer_clock_and_reject_false_attribution():
    metrics = {
        "schema_version": 4,
        "system": "quanta",
        "route_count": 1,
        "query_schedule": ["T1"],
        "warmup_passes": 0,
        "measurement_repetitions": 1,
        "query_timing": {
            "boundary": semble.QUERY_TIMING_BOUNDARY,
            "clock": semble.QUERY_TIMING_CLOCK,
            "observations": [
                {
                    "task_id": "T1",
                    "route": "lexical",
                    "phase": "measured",
                    "iteration": 0,
                    "start_ns": 10,
                    "end_ns": 110,
                    "status": "success",
                    "output_bytes": 1,
                    "sdk_execute_ns": 30,
                    "sdk_post_execute_ns": 20,
                    "runner_result_materialize_ns": 10,
                }
            ],
        },
    }
    pairrun.validate_completed_query_timing(metrics)
    for key in ("sdk_execute_ns", "sdk_post_execute_ns", "runner_result_materialize_ns"):
        for value in (-1, True, 1 << 64, 101):
            mutant = copy.deepcopy(metrics)
            mutant["query_timing"]["observations"][0][key] = value
            with pytest.raises(pairrun.RunError, match="SDK child clocks"):
                pairrun.validate_completed_query_timing(mutant)
        mutant = copy.deepcopy(metrics)
        del mutant["query_timing"]["observations"][0][key]
        with pytest.raises(pairrun.RunError, match="observation is malformed"):
            pairrun.validate_completed_query_timing(mutant)
    # Historical phase contracts cannot silently acquire new attribution fields.
    historical = copy.deepcopy(metrics)
    historical["schema_version"] = 3
    with pytest.raises(pairrun.RunError, match="observation is malformed"):
        pairrun.validate_completed_query_timing(historical)


def test_completed_response_qualification_accepts_verified_fresh_release_source(tmp_path):
    stage = fixtures._pair_stage(
        tmp_path,
        repetitions=5,
        qualified_speed_sample=True,
        scope="qualified",
        claims={"speed": True},
        sdk_build_profile="release-fresh",
    )
    _add_timing(stage)
    verdict = fixtures._stage_verdict(stage)
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["states"]["SDK_PATH_GREEN"] == "pass", verdict["state_evidence"][
        "SDK_PATH_GREEN"
    ]
    assert verdict["states"]["PERF_QUALIFIED"] == "pass", verdict["state_evidence"][
        "PERF_QUALIFIED"
    ]
    assert (
        verdict["provenance"]["quanta"]["binary_build_source_revision"]
        == (stage["manifest"]["provenance"]["quanta"]["source_sha"])
    )


@pytest.mark.parametrize("system", ["quanta", "semble"])
@pytest.mark.parametrize("scope", ["exploratory", "qualified"])
def test_phase_digest_rejects_byte_tamper_with_identical_parsed_metrics(tmp_path, system, scope):
    stage = fixtures._pair_stage(
        tmp_path,
        repetitions=5,
        qualified_speed_sample=True,
        scope=scope,
        claims={"speed": True},
    )
    _add_timing(stage)
    assert fixtures._stage_verdict(stage)["states"]["PAIR_VALID"] == "pass"
    manifest = pairrun.read_json(stage["manifest_path"])
    target = next(ref for ref in manifest["artifacts"]["phase_metrics"] if f"/{system}/" in ref)
    path = stage["stage"] / target
    before = pairrun.read_json(path)
    path.write_bytes(path.read_bytes() + b"\n")
    assert pairrun.read_json(path) == before
    verdict = fixtures._stage_verdict(stage)
    assert verdict["states"]["PERF_QUALIFIED"] == (
        "fail" if scope == "qualified" else "not_applicable"
    )
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["proof_digest"] is None
    assert "phase metrics digest differs" in verdict["state_evidence"]["PAIR_VALID"]["reason"]


@pytest.mark.parametrize(
    "mutate",
    [
        lambda artifacts: artifacts.pop("phase_metrics_digests"),
        lambda artifacts: artifacts["phase_metrics_digests"].clear(),
        lambda artifacts: artifacts["phase_metrics_digests"].update({"foreign.json": "0" * 64}),
        lambda artifacts: artifacts["phase_metrics_digests"].update(
            {artifacts["phase_metrics"][0]: "bad"}
        ),
        lambda artifacts: artifacts["phase_metrics"].append(artifacts["phase_metrics"][0]),
    ],
)
def test_phase_digest_map_requires_exact_capture_inventory(tmp_path, mutate):
    stage = fixtures._pair_stage(tmp_path)
    manifest = pairrun.read_json(stage["manifest_path"])
    mutate(manifest["artifacts"])
    with pytest.raises(pairrun.RunError, match="artifacts|phase_metrics_digests"):
        pairrun._validate_manifest_shape(manifest)


@pytest.mark.parametrize(
    "mutate,match",
    [
        (lambda timing: timing.update(boundary="library_dispatch"), "boundary"),
        (lambda timing: timing.update(clock="worker_minus_parent"), "clock"),
        (lambda timing: timing["observations"].pop(), "schedule"),
        (lambda timing: timing["observations"][0].update(output_bytes=0), "output"),
        (lambda timing: timing["observations"][0].update(end_ns=-1), "monotonic"),
        (lambda timing: timing["observations"][-1].update(end_ns=9000000), "sample"),
        (lambda timing: timing["observations"][-1].update(status="timeout"), "failed"),
    ],
)
def test_completed_clock_refuses_partial_or_mismatched_proof(mutate, match):
    protocol = pairrun.build_query_protocol(["T1"], 0, 1, 1)
    phase = {
        "route_count": 1,
        "query_protocol": protocol,
        "warm_latencies_ms": {"lexical": {"T1": [1.5]}},
        "cold_latencies_ms": {"lexical": 1.0},
    }
    record = {
        "results": [
            {
                "task_id": "T1",
                "route": "lexical",
                "status": "success",
                "timings": {"query_latency_ms": 1.5},
                "candidates": [],
            }
        ]
    }
    phase["query_timing"] = _timing(phase, record)
    pairrun.validate_completed_query_timing(phase, record)
    bad = copy.deepcopy(phase)
    mutate(bad["query_timing"])
    with pytest.raises(pairrun.RunError, match=match):
        pairrun.validate_completed_query_timing(bad, record)


def test_completed_clock_accepts_suite_route_order_and_capped_responses():
    routes = ("lexical", "semantic", "hybrid")
    observations = [
        {
            "task_id": "T1",
            "route": route,
            "phase": "measured",
            "iteration": 0,
            "start_ns": index * 1_000_000,
            "end_ns": (index + 1) * 1_000_000,
            "status": "capped" if route == "lexical" else "success",
            "output_bytes": 1,
        }
        for index, route in enumerate(routes)
    ]
    phase = {
        "route_count": 3,
        "query_schedule": ["T1"],
        "warmup_passes": 0,
        "measurement_repetitions": 1,
        "query_timing": {
            "boundary": semble.QUERY_TIMING_BOUNDARY,
            "clock": semble.QUERY_TIMING_CLOCK,
            "observations": observations,
        },
    }
    record = {
        "results": [
            {
                "task_id": "T1",
                "route": row["route"],
                "status": row["status"],
                "timings": {"query_latency_ms": 1.0},
            }
            for row in observations
        ]
    }
    pairrun.validate_completed_query_timing(phase, record)
    reordered = copy.deepcopy(phase)
    reordered["query_timing"]["observations"][1]["route"] = "lexical"
    with pytest.raises(pairrun.RunError, match="route inventory"):
        pairrun.validate_completed_query_timing(reordered, record)


def test_semble_parent_clock_ends_after_real_normalization(tmp_path, monkeypatch):
    fixtures._write_stub_semble(tmp_path)
    worker = tmp_path / "worker.py"
    worker.write_text(semble.WORKER_TEMPLATE)
    spec = {
        "corpus_dir": str(tmp_path),
        "tasks": [{"task_id": "T1", "query": "q"}],
        "top_k": 5,
        "seed": 0,
        "warmup_passes": 0,
        "repetitions": 1,
        "execution_profile_sha256": pairrun.digest(
            pairrun.canonical(semble.execution_profile("native-default", None))
        ),
    }
    spec_path, native_path = tmp_path / "spec.json", tmp_path / "native.json"
    spec_path.write_text(json.dumps(spec))
    monkeypatch.setenv("SPEC_JSON", str(spec_path))
    monkeypatch.setenv("NATIVE_JSON", str(native_path))
    monkeypatch.setenv("SEMBLE_MODEL_NAME", "stub")
    monkeypatch.setenv("PYTHONPATH", str(tmp_path))
    clock = {"now": 1000}
    monkeypatch.setattr(semble.time, "monotonic_ns", lambda: clock["now"])

    def normalization_work():
        clock["now"] += 7_000_000

    completed = fixtures._run_protocol_worker_fixture(
        worker, spec, normalization_hook=normalization_work
    )
    assert completed.returncode == 0, completed.stderr
    native = pairrun.read_json(native_path)
    observation = native["query_timing"]["observations"][0]
    assert observation["start_ns"] == 0
    assert observation["end_ns"] == 7_000_000
    assert observation["status"] == "success"
    assert observation["output_bytes"] > 0
    assert native["latencies_ms"] == {"T1": [7.0]}


def test_completed_worker_deadline_covers_a_partial_protocol_line(tmp_path):
    worker = tmp_path / "partial.py"
    worker.write_text(
        "import sys,time\nsys.stdout.write('{'); sys.stdout.flush(); time.sleep(60)\n"
    )
    started = time.monotonic()
    with pytest.raises(semble.AdapterError, match="timed out"):
        semble.run_completed_worker(
            [sys.executable, str(worker)],
            env=dict(os.environ),
            timeout_secs=0.2,
            tasks={"T1": "q"},
            top_k=1,
            route="lexical",
            normalize_response=lambda *args: pytest.fail("partial message cannot complete"),
            stderr_path=tmp_path / "partial.stderr",
        )
    assert time.monotonic() - started < 2
