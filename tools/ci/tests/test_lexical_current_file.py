"""Current file-unit capture uses the public record merge and evaluator contracts."""

import copy
import json
import subprocess

import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import lexical_file_comparison as owner
from tools.benchmark.retrieval import query_plan, run
from tools.benchmark.retrieval import semble as semble_adapter
from tools.ci.tests.test_retrieval_benchmark import _file_projection_run, _v3_capture


def write(path, value):
    path.write_bytes(ev.canonical(value))
    return path


@pytest.fixture
def current_inputs(tmp_path, request):
    repo, suite, q, _, _ = _file_projection_run(
        tmp_path / "corpus", "code_search_file", queries=["alphaTwo", "alphaThree"]
    )
    template_task, template_row = suite["tasks"][0], q["results"][0]
    suite["tasks"] = []
    q["results"] = []
    for index in range(20):
        task = copy.deepcopy(template_task)
        task["task_id"] = f"F{index:02d}"
        task["query"] = [
            "alpha",
            "bravo",
            "charlie",
            "delta",
            "echo",
            "foxtrot",
            "golf",
            "hotel",
            "india",
            "juliet",
            "kilo",
            "lima",
            "mike",
            "november",
            "oscar",
            "papa",
            "quebec",
            "romeo",
            "sierra",
            "tango",
        ][index]
        task["query_sha256"] = ev.digest(task["query"].encode())
        task["query_family_id"] = f"fam-{index}"
        row = copy.deepcopy(template_row)
        row["task_id"] = task["task_id"]
        row["query_identity"] = query_plan.derive_query_identity("code_search_file", task["query"])
        row["score_evidence"] = "native_sdk_score_v1"
        for rank, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - rank)
        suite["tasks"].append(task)
        q["results"].append(row)
    if getattr(request, "param", None) == "no_answer":
        for task in suite["tasks"]:
            task.update(answerable=False, gold=[], file_judgments=[])
    suite["routes"] = owner.FILE_ROUTES
    suite, pack, _ = ev.validate_suite(repo, suite)
    s = copy.deepcopy(q)
    s.pop("span_accounting_version")
    profile = semble_adapter.execution_profile("lexical-file", None)
    s["captures"] = {"s0": _v3_capture("semble", current=True)}
    s["captures"]["s0"]["execution_profile"] = profile
    s["captures"]["s0"]["execution_profile_sha256"] = ev.digest(ev.canonical(profile))
    s["route_provenance"] = {"semble-lexical-file": {"capture_id": "s0"}}
    for task, row in zip(suite["tasks"], s["results"], strict=True):
        row["route"] = "semble-lexical-file"
        row["ordering"] = "score_desc_native_tiebreak"
        row["score_evidence"] = "semble_bm25_score_v1"
        row["query_identity"] = {
            "original_query_sha256": task["query_sha256"],
            "submitted_query_sha256": task["query_sha256"],
        }
        row["file_collection"] = {
            "indexed_chunks": 2,
            "matched_chunks": len(row["candidates"]),
            "matching_files": len(row["candidates"]),
        }
        for candidate in row["candidates"]:
            candidate.pop("span_accounting")
            candidate["tokens"] = len(
                ev.TOKEN_RE.findall((repo / candidate["path"]).read_bytes().decode())
            )
    for record, routes in ((q, ["lexical"]), (s, ["semble-lexical-file"])):
        narrowed, _ = run.project_pack_and_suite(pack, suite, routes)
        record["query_pack_sha256"] = ev.digest(ev.canonical(narrowed))
    paths = {role: tmp_path / (role + ".json") for role in owner.FILE_INPUT_ROLES}
    write(paths["suite"], suite)
    write(paths["query_pack"], pack)
    write(paths["quanta_record"], q)
    write(paths["semble_record"], s)
    _, _, merged = run.merge_records(
        repo, paths["suite"], [paths["quanta_record"], paths["semble_record"]]
    )
    report = ev.evaluate_paired_file_diagnostic(
        suite, pack, merged, "semble-lexical-file", "lexical"
    )
    write(paths["pair_report"], report)
    profiles = {"quanta": q["captures"]["q0"]["execution_profile"], "semble": profile}
    lock = {
        "execution_profiles": profiles,
        "execution_profiles_sha256": ev.digest(ev.canonical(profiles)),
        "quanta_routes": ["lexical"],
        "semble_route": "semble-lexical-file",
        "top_k": 10,
        "suite_digest": owner._sha(paths["suite"]),
        "query_pack_digest": owner._sha(paths["query_pack"]),
        "searchd_expected_sha256": q["captures"]["q0"]["searchd_binary"]["binary_digest"],
        "semble_lockfile_sha256": "e" * 64,
        "strategies": ["whole_file"],
    }
    write(paths["pair_lock"], lock)
    provenance = {
        "quanta": {
            "binary_digest": q["captures"]["q0"]["runner_binary"]["digest"],
            "source_sha": "a" * 40,
        },
        "suite": {
            "suite_digest": lock["suite_digest"],
            "query_pack_digest": lock["query_pack_digest"],
        },
        "semble": {"lockfile_digest": "e" * 64},
    }
    write(paths["pair_manifest"], {"provenance": provenance})
    write(
        paths["pair_verdict"],
        {
            "provenance": provenance,
            "comparisons": [
                {
                    "candidate_route": "lexical",
                    "baseline_route": "semble-lexical-file",
                    "strategy": "whole_file",
                    "report_digest": owner._sha(paths["pair_report"]),
                    "record_digest": ev.digest(ev.canonical(merged)),
                }
            ],
            "counts": {"selected": 40, "executed": 40, "passed": 40, "failed": 0},
            "states": {"PAIR_VALID": "pass"},
        },
    )
    write(
        paths["semble_native"],
        {
            "semble_profile": "lexical-file",
            "rerank_applied": False,
            "lane_call_counts": {"bm25": 20, "semantic": 0, "encode": 0},
            "execution_events": [
                {
                    "task_id": task["task_id"],
                    "phase": "measured",
                    "lane_entry_counts": {"bm25": 1, "semantic": 0},
                    "profile_sha256": ev.digest(ev.canonical(profile)),
                    "submitted_query_sha256": task["query_sha256"],
                }
                for task in pack["tasks"]
            ],
        },
    )
    phase_refs = [f"rep-00/{system}/phase-metrics.json" for system in ("quanta", "semble")]
    phase_digests = {}
    for system, route, record in (("quanta", "lexical", q), ("semble", "semble-lexical-file", s)):
        observations = [
            {
                "task_id": task["task_id"],
                "route": route,
                "phase": "measured",
                "iteration": 0,
                "start_ns": index * 1_500_000,
                "end_ns": (index + 1) * 1_500_000,
                "status": "success",
                "output_bytes": 100,
            }
            for index, task in enumerate(pack["tasks"])
        ]
        timing = {
            "boundary": "request_construction_to_normalized_response",
            "clock": "capture_relative_monotonic_ns",
            "observations": observations,
        }
        phases = {
            "discovery": 1.0,
            "model_provider_prepare": 1.0,
            "first_query": 1.5,
            "warm_query": 28.5,
            "unattributed": 1.0,
        }
        if system == "quanta":
            phases.update(chunk=1.0, embed_publish_seal_activate=1.0)
        else:
            phases.update(index=1.0, warmup=0.0)
        phase = {
            "schema_version": 1,
            "system": system,
            "timing_layer": "runner_monotonic_wall_v1"
            if system == "quanta"
            else "worker_monotonic_wall_v1",
            "strategy": "whole_file",
            "record_sha256": owner._sha(paths[system + "_record"]),
            "runner_binary_sha256" if system == "quanta" else "worker_sha256": record["captures"][
                "q0" if system == "quanta" else "s0"
            ]["runner_binary"]["digest"],
            "task_count": 20,
            "route_count": 1,
            "file_count": 2,
            "chunk_count": 2,
            "query_schedule": [task["task_id"] for task in pack["tasks"]],
            "warmup_passes": 0,
            "measurement_repetitions": 1,
            "query_timing": timing,
            "phases_ms": phases,
            "total_ms": sum(phases.values()),
        }
        if system == "semble":
            phase["phase_boundaries_ns"] = {
                "worker_start": 0,
                "discovery_end": 1_000_000,
                "model_provider_prepare_end": 2_000_000,
                "index_end": 3_000_000,
                "warmup_end": 3_000_000,
                "query_start": 3_000_000,
                "first_query_start": 3_000_000,
                "first_query_end": 4_500_000,
                "query_end": 33_000_000,
                "worker_end": 34_000_000,
            }
            native = json.loads(paths["semble_native"].read_bytes())
            native["query_timing"] = timing
            write(paths["semble_native"], native)
        write(paths[system + "_phase_metrics"], phase)
        phase_digests[f"rep-00/{system}/phase-metrics.json"] = owner._sha(
            paths[system + "_phase_metrics"]
        )
    manifest = json.loads(paths["pair_manifest"].read_bytes())
    manifest["artifacts"] = {"phase_metrics": phase_refs, "phase_metrics_digests": phase_digests}
    write(paths["pair_manifest"], manifest)
    subprocess.run(
        ["git", "-C", str(repo), "bundle", "create", str(paths["source_bundle"]), "--all"],
        check=True,
        capture_output=True,
    )
    for product in owner.PRODUCTS:
        rows = []
        for task in suite["tasks"]:
            gold = sorted({row["path"] for row in task["gold"]})
            row = {
                "lane": "symbol_only",
                "task_id": task["task_id"],
                "submitted_query": task["query"],
                "gold_paths": gold,
                "file_hit_at_10": bool(set(gold) & {"a.txt", "b.txt"}),
                "elapsed_ms": 2.0,
            }
            if product == "cs":
                row.update(paths=["a.txt", "b.txt"], exit_code=0)
            else:
                row.update(file_paths_top_10=["a.txt", "b.txt"], http_status=200, error=None)
                if product == "opengrok":
                    row["field"] = "full"
            rows.append(row)
        paths[product + "_rows"].write_bytes(b"".join(ev.canonical(row) + b"\n" for row in rows))
    return paths


def test_current_pair_replays_raw_file_results(current_inputs):
    paths = current_inputs
    result = owner.file_pair_result(
        paths, paths["suite"].read_bytes(), paths["query_pack"].read_bytes()
    )
    assert set(result["routes"]) == {"quanta_lexical", "semble_lexical_file"}
    for route in result["routes"].values():
        assert route["rank_unit"] == "distinct_file"
        assert len(route["per_query"]) == 20
        assert all(row["file_hit_at_10"] is True for row in route["per_query"])
        assert route["latency_ms"]["count"] == 20
        assert route["latency_ms"]["timing_layer"] == "request_construction_to_normalized_response"


def test_current_five_product_contract_and_common_denominator(current_inputs, tmp_path):
    from tools.ci.tests.test_lexical_capture import capture

    paths = current_inputs
    summary = owner.evaluate_capture(paths)
    assert summary["common_eligible_tasks"] == 20
    assert set(summary["common_eligible_products"]) == set(capture.FILE_PRODUCTS)
    assert all(row["file_hit_at_10"] == 1.0 for row in summary["common_eligible_products"].values())
    typed = capture.payloads(
        summary,
        json.loads(paths["suite"].read_bytes()),
        json.loads(paths["query_pack"].read_bytes()),
    )
    assert set(typed) == set(capture.FILE_PRODUCTS)
    assert all(row["value"] == 1.0 for product in typed.values() for row in product["rows"])
    spec = write(
        tmp_path / "spec.json",
        {"schema_version": 1, **{key: str(value) for key, value in paths.items()}},
    )
    output = tmp_path / "five-product.json"
    import sys

    subprocess.run(
        [
            sys.executable,
            "-m",
            "tools.benchmark.retrieval.lexical_five_product_oracle",
            "--spec",
            str(spec),
            "--out",
            str(output),
        ],
        check=True,
        capture_output=True,
    )
    result = json.loads(output.read_bytes())
    assert result["pair"]["runner_record_sha256"] == summary["pair"]["runner_record_sha256"]
    assert result["common_eligible_products"] == summary["common_eligible_products"]


@pytest.mark.parametrize(
    "change",
    [
        "missing_raw",
        "swapped_profile",
        "swapped_source",
        "malformed_status",
        "changed_path",
        "wrong_query_binding",
    ],
)
def test_current_pair_refuses_unbound_or_malformed_raw(current_inputs, change):
    paths = current_inputs
    if change == "missing_raw":
        paths = {key: value for key, value in paths.items() if key != "quanta_record"}
    elif change == "swapped_source":
        # A valid bundle containing a different selected corpus revision must not substitute.
        source = paths["source_bundle"].parent / "other"
        source.mkdir()
        subprocess.run(["git", "init", "-q", str(source)], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                str(source),
                "-c",
                "user.email=t@x",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "different corpus",
            ],
            check=True,
        )
        paths["source_bundle"].unlink()
        subprocess.run(
            ["git", "-C", str(source), "bundle", "create", str(paths["source_bundle"]), "--all"],
            check=True,
            capture_output=True,
        )
    else:
        role = (
            "pair_lock" if change in {"swapped_profile", "wrong_query_binding"} else "quanta_record"
        )
        value = json.loads(paths[role].read_bytes())
        if change == "swapped_profile":
            value["execution_profiles"]["quanta"] = {"policy": "native"}
        elif change == "wrong_query_binding":
            value["query_pack_digest"] = "0" * 64
        elif change == "malformed_status":
            value["results"][0]["status"] = "invented"
        else:
            value["results"][0]["candidates"][0]["path"] = "outside.txt"
        write(paths[role], value)
    with pytest.raises((ValueError, ev.EvidenceError)):
        owner.file_pair_result(paths, paths["suite"].read_bytes(), paths["query_pack"].read_bytes())


@pytest.mark.parametrize(
    "change",
    ["phase_hash", "clock", "status", "latency", "missing_timing", "missing_role", "native_timing"],
)
def test_completed_response_requires_bound_raw_phase_proof(current_inputs, change):
    paths = current_inputs
    if change == "missing_role":
        paths = {key: value for key, value in paths.items() if key != "quanta_phase_metrics"}
    elif change == "native_timing":
        native = json.loads(paths["semble_native"].read_bytes())
        native["query_timing"]["clock"] = "different_clock"
        write(paths["semble_native"], native)
    else:
        phase = json.loads(paths["quanta_phase_metrics"].read_bytes())
        if change in {"phase_hash", "latency"}:
            phase["query_timing"]["observations"][0]["end_ns"] -= 1000
        elif change == "clock":
            phase["query_timing"]["clock"] = "different_clock"
        elif change == "status":
            phase["query_timing"]["observations"][0]["status"] = "abstained"
        else:
            phase.pop("query_timing")
        write(paths["quanta_phase_metrics"], phase)
        if change != "phase_hash":
            # Updating the outer digest cannot make malformed clock/status/sample proof valid.
            manifest = json.loads(paths["pair_manifest"].read_bytes())
            manifest["artifacts"]["phase_metrics_digests"]["rep-00/quanta/phase-metrics.json"] = (
                owner._sha(paths["quanta_phase_metrics"])
            )
            write(paths["pair_manifest"], manifest)
    with pytest.raises((ValueError, ev.EvidenceError)):
        owner.file_pair_result(paths, paths["suite"].read_bytes(), paths["query_pack"].read_bytes())


def test_completed_response_diagnostic_preserves_capped_status(current_inputs):
    paths = current_inputs
    q = json.loads(paths["quanta_record"].read_bytes())
    q["results"][0]["status"] = "capped"
    write(paths["quanta_record"], q)
    phase = json.loads(paths["quanta_phase_metrics"].read_bytes())
    phase["record_sha256"] = owner._sha(paths["quanta_record"])
    phase["query_timing"]["observations"][0]["status"] = "capped"
    write(paths["quanta_phase_metrics"], phase)
    manifest = json.loads(paths["pair_manifest"].read_bytes())
    manifest["artifacts"]["phase_metrics_digests"]["rep-00/quanta/phase-metrics.json"] = owner._sha(
        paths["quanta_phase_metrics"]
    )
    write(paths["pair_manifest"], manifest)
    repo = paths["suite"].parent / "corpus/source"
    suite, pack, merged = run.merge_records(
        repo, paths["suite"], [paths["quanta_record"], paths["semble_record"]]
    )
    report = ev.evaluate_paired_file_diagnostic(
        suite, pack, merged, "semble-lexical-file", "lexical"
    )
    write(paths["pair_report"], report)
    verdict = json.loads(paths["pair_verdict"].read_bytes())
    verdict["comparisons"][0]["report_digest"] = owner._sha(paths["pair_report"])
    verdict["comparisons"][0]["record_digest"] = ev.digest(ev.canonical(merged))
    write(paths["pair_verdict"], verdict)
    result = owner.file_pair_result(
        paths, paths["suite"].read_bytes(), paths["query_pack"].read_bytes()
    )
    assert result["routes"]["quanta_lexical"]["per_query"][0]["status"] == "capped"
    assert (
        result["routes"]["quanta_lexical"]["latency_ms"]["timing_layer"]
        == "request_construction_to_normalized_response"
    )
    run.validate_completed_query_timing(phase, q)
    incomplete = copy.deepcopy(phase)
    incomplete["query_timing"]["observations"][0]["status"] = "timeout"
    with pytest.raises(run.RunError, match="incomplete or failed"):
        run.validate_completed_query_timing(incomplete, q)


@pytest.mark.parametrize("current_inputs", ["no_answer"], indirect=True)
def test_no_answer_current_file_rows_keep_candidate_observations(current_inputs):
    from tools.ci.tests.test_lexical_capture import capture

    paths = current_inputs
    summary = owner.evaluate_capture(paths)
    typed = capture.payloads(
        summary,
        json.loads(paths["suite"].read_bytes()),
        json.loads(paths["query_pack"].read_bytes()),
    )
    for product in typed.values():
        assert len(product["rows"]) == 20
        assert all(
            row["metric"] == "no_gold_empty_at_10"
            and row["state"] == "no_answer"
            and row["value"] == 0.0
            for row in product["rows"]
        )
    for route in summary["pair"]["routes"].values():
        assert all(
            row["answerable"] is False and row["candidates"] == 2 for row in route["per_query"]
        )
        assert route["latency_ms"]["timing_layer"] == "request_construction_to_normalized_response"


@pytest.mark.parametrize("claimed_passed", [39, 40])
def test_abstention_uses_canonical_terminal_counts(current_inputs, claimed_passed):
    paths = current_inputs
    record = json.loads(paths["semble_record"].read_bytes())
    row = record["results"][0]
    row.update(status="abstained", candidates=[])
    row["file_collection"].update(matched_chunks=0, matching_files=0)
    write(paths["semble_record"], record)
    phase = json.loads(paths["semble_phase_metrics"].read_bytes())
    phase["record_sha256"] = owner._sha(paths["semble_record"])
    phase["query_timing"]["observations"][0]["status"] = "abstained"
    write(paths["semble_phase_metrics"], phase)
    native = json.loads(paths["semble_native"].read_bytes())
    native["query_timing"] = phase["query_timing"]
    write(paths["semble_native"], native)
    manifest = json.loads(paths["pair_manifest"].read_bytes())
    manifest["artifacts"]["phase_metrics_digests"]["rep-00/semble/phase-metrics.json"] = owner._sha(
        paths["semble_phase_metrics"]
    )
    write(paths["pair_manifest"], manifest)
    repo = paths["suite"].parent / "corpus/source"
    suite, pack, merged = run.merge_records(
        repo, paths["suite"], [paths["quanta_record"], paths["semble_record"]]
    )
    write(
        paths["pair_report"],
        ev.evaluate_paired_file_diagnostic(suite, pack, merged, "semble-lexical-file", "lexical"),
    )
    verdict = json.loads(paths["pair_verdict"].read_bytes())
    verdict["counts"]["passed"] = claimed_passed
    verdict["comparisons"][0]["report_digest"] = owner._sha(paths["pair_report"])
    verdict["comparisons"][0]["record_digest"] = ev.digest(ev.canonical(merged))
    write(paths["pair_verdict"], verdict)
    if claimed_passed == 40:
        with pytest.raises(ValueError, match="terminal counts"):
            owner.file_pair_result(
                paths, paths["suite"].read_bytes(), paths["query_pack"].read_bytes()
            )
    else:
        result = owner.file_pair_result(
            paths, paths["suite"].read_bytes(), paths["query_pack"].read_bytes()
        )
        assert result["routes"]["semble_lexical_file"]["per_query"][0]["status"] == "abstained"
