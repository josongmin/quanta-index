"""Current file-unit capture uses the public record merge and evaluator contracts."""

import copy
import json
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import lexical_file_comparison as owner
from tools.benchmark.retrieval import run


def write(path, value):
    path.write_bytes(ev.canonical(value))
    return path


@pytest.fixture
def current_inputs(tmp_path, request):
    # Build the complete canonical pair stage. The current-file replay validates
    # the same manifest through run.build_verdict, so a two-key synthetic
    # manifest would never reach the raw-record assertions below.
    from tools.ci.tests.test_retrieval_benchmark import _pair_stage

    status = getattr(request, "param", None)
    if status not in (None, "no_answer", "capped", "abstained"):
        raise ValueError(f"unsupported current file fixture state: {status}")
    stage = _pair_stage(
        tmp_path / "canonical",
        file_current=True,
        file_status=status,
        diagnostic_version=8,
    )
    layout = stage["rep_layouts"][0]
    stage_root = stage["stage"]
    suite = json.loads(stage["suite_path"].read_bytes())
    report_refs = stage["manifest"]["artifacts"]["reports"]
    assert len(report_refs) == 1
    paths = {
        "suite": stage["suite_path"],
        "query_pack": stage_root / "query-pack.json",
        "pair_report": stage_root / report_refs[0],
        "pair_lock": stage_root / "protocol-lock.json",
        "semble_native": stage_root / "rep-00" / "semble" / "native.json",
        "pair_verdict": stage_root / "verdict.json",
        "quanta_record": Path(layout["quanta"]["whole_file"]),
        "semble_record": Path(layout["semble"]),
        "source_bundle": tmp_path / "source.bundle",
        "pair_manifest": stage["manifest_path"],
        "quanta_phase_metrics": Path(layout["quanta_phase_metrics"]["whole_file"]),
        "semble_phase_metrics": Path(layout["semble_phase_metrics"]),
        **{product + "_rows": tmp_path / (product + "_rows.jsonl") for product in owner.PRODUCTS},
    }
    write(
        paths["pair_verdict"],
        run.build_verdict(stage["repo"], stage["suite_path"], stage["manifest_path"]),
    )
    subprocess.run(
        ["git", "-C", str(stage["repo"]), "bundle", "create", str(paths["source_bundle"]), "--all"],
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
    assert set(json.loads(paths["pair_manifest"].read_bytes())) == {
        "manifest_version",
        "blinding",
        "isolation_method",
        "access_block_log",
        "scope",
        "claims",
        "repetitions",
        "evidence",
        "host",
        "artifacts",
        "provenance",
    }
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


def test_current_file_verdict_refuses_typed_alias_and_source_closure_tamper(current_inputs):
    from tools.benchmark.retrieval.lexical_file_comparison import _replay_file_pair_verdict

    paths = current_inputs
    repo = paths["pair_manifest"].parent.parent / "src" / "source"
    original = json.loads(paths["pair_verdict"].read_bytes())
    _replay_file_pair_verdict(repo, paths["suite"], paths["pair_manifest"], original)
    for key, replacement in (("selected", 40.0), ("passed", True)):
        mutant = copy.deepcopy(original)
        mutant["counts"][key] = replacement
        with pytest.raises(ValueError, match="verdict differs from canonical replay"):
            _replay_file_pair_verdict(repo, paths["suite"], paths["pair_manifest"], mutant)
    manifest = json.loads(paths["pair_manifest"].read_bytes())
    closure = paths["pair_manifest"].parent / manifest["artifacts"]["driver_source_closure"]
    # Closure custody is canonical JSON: formatting alone is not a source mutation.
    closure.write_bytes(closure.read_bytes() + b" ")
    _replay_file_pair_verdict(repo, paths["suite"], paths["pair_manifest"], original)
    changed = json.loads(closure.read_bytes())
    assert changed["files"][0]["sha256"] != "0" * 64
    changed["files"][0]["sha256"] = "0" * 64
    core = {
        key: changed[key] for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    changed["digest"] = ev.digest(ev.canonical(core))
    assert changed["digest"] != manifest["provenance"]["quanta"]["source_closure_digest"]
    write(closure, changed)
    with pytest.raises(ValueError, match="verdict differs from canonical replay"):
        _replay_file_pair_verdict(repo, paths["suite"], paths["pair_manifest"], original)


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
            ref = next(
                ref for ref in manifest["artifacts"]["phase_metrics"] if "quanta" in Path(ref).parts
            )
            manifest["artifacts"]["phase_metrics_digests"][ref] = owner._sha(
                paths["quanta_phase_metrics"]
            )
            write(paths["pair_manifest"], manifest)
    with pytest.raises((ValueError, ev.EvidenceError)):
        owner.file_pair_result(paths, paths["suite"].read_bytes(), paths["query_pack"].read_bytes())


@pytest.mark.parametrize("current_inputs", ["capped"], indirect=True)
def test_completed_response_diagnostic_preserves_capped_status(current_inputs):
    paths = current_inputs
    q = json.loads(paths["quanta_record"].read_bytes())
    phase = json.loads(paths["quanta_phase_metrics"].read_bytes())
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
    for observation in incomplete["query_timing"]["observations"]:
        if (
            observation["phase"] == "measured"
            and observation["task_id"] == q["results"][0]["task_id"]
        ):
            observation["status"] = "timeout"
            break
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


@pytest.mark.parametrize("current_inputs", ["abstained"], indirect=True)
@pytest.mark.parametrize("claimed_passed", [39, 40])
def test_abstention_uses_canonical_terminal_counts(current_inputs, claimed_passed):
    paths = current_inputs
    verdict = json.loads(paths["pair_verdict"].read_bytes())
    assert verdict["counts"]["passed"] == 39
    if claimed_passed == 40:
        verdict["counts"]["passed"] = 40
        write(paths["pair_verdict"], verdict)
        with pytest.raises(ValueError, match="verdict differs from canonical replay"):
            owner.file_pair_result(
                paths, paths["suite"].read_bytes(), paths["query_pack"].read_bytes()
            )
    else:
        result = owner.file_pair_result(
            paths, paths["suite"].read_bytes(), paths["query_pack"].read_bytes()
        )
        assert result["routes"]["semble_lexical_file"]["per_query"][0]["status"] == "abstained"
