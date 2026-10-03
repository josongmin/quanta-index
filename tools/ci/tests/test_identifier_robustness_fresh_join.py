"""Fixed input and route guards for the fresh OSA1 five-product join."""

import copy
import json
import subprocess

import pytest

from tools.benchmark.retrieval import identifier_robustness_fresh_join as fresh


def _profiles():
    native = {
        "candidate_route": "lexical",
        "baseline_route": "semble-lexical-file",
        "execution_profiles": {
            "quanta": {
                "config": {},
                "planning_cost_in_latency": False,
                "policy": "code_search_file",
                "profile_id": "quanta-code-search-file-v1",
            },
            "semble": {"mode": "lexical-file"},
        },
        "top_k": 10,
    }
    external = {
        key: {}
        for key in (
            "corpus",
            "cs",
            "opengrok",
            "output_root",
            "query_pack",
            "schema_version",
            "sourcegraph",
            "suite",
        )
    }
    native["execution_profiles"]["semble"] = {
        "alpha": None,
        "mode": "lexical-file",
        "profile_id": "semble-lexical-file-v1",
        "rerank": "not_applicable",
    }
    external["sourcegraph"] = {"repository": "benchmark/fixture"}
    external["opengrok"] = {"project": "fixture", "indexed_view_probe": "full"}
    external["cs"] = {"binary": "cs"}
    return native, external


def test_fresh_join_accepts_only_default_file_request_profiles():
    native, external = _profiles()
    fresh._require_default_profiles(native, external, "fixture")
    typo = copy.deepcopy(native)
    typo["execution_profiles"]["quanta"]["policy"] = "code_search_typo_file"
    with pytest.raises(fresh.FreshJoinError, match="not default file search"):
        fresh._require_default_profiles(typo, external, "fixture")
    fuzzy = copy.deepcopy(external)
    fuzzy["capability"] = "cs_fuzzy_osa1_file"
    with pytest.raises(fresh.FreshJoinError, match="not default file search"):
        fresh._require_default_profiles(native, fuzzy, "fixture")
    truncated = copy.deepcopy(native)
    truncated["top_k"] = 100
    with pytest.raises(fresh.FreshJoinError, match="not default file search"):
        fresh._require_default_profiles(truncated, external, "fixture")
    wrong_field = copy.deepcopy(external)
    wrong_field["opengrok"]["indexed_view_probe"] = "none"
    with pytest.raises(fresh.FreshJoinError, match="not default file search"):
        fresh._require_default_profiles(native, wrong_field, "fixture")


def test_fresh_join_repaired_cell_preserves_original_cohort():
    blocked = [{"repository": "repo", "tasks": 5, "status": "BLOCKED"}]
    fingerprints = {
        "blind_pack_sha256": "pack",
        "task_identity_sha256": "tasks",
        "file_universe_sha256": "files",
    }
    custody = [{"source_blocked_originals": {"repo": fingerprints}}]
    assert fresh._reconcile_blocked(blocked, [], custody) == (blocked, [])
    repaired = [{"repository": "repo", **fingerprints}]
    assert fresh._reconcile_blocked(blocked, repaired, custody) == ([], blocked)
    for key in fingerprints:
        changed = [{**repaired[0], key: "drift"}]
        with pytest.raises(fresh.FreshJoinError, match="changes the fixed original query cohort"):
            fresh._reconcile_blocked(blocked, changed, custody)


def test_fresh_join_binds_raw_record_to_scored_pair_report():
    suite = {"tasks": [{"task_id": "q1"}], "routes": ["lexical", "semble-lexical-file"]}
    pack = {"tasks": [{"task_id": "q1"}]}
    merged = {
        "route_provenance": {
            "lexical": {"capture_id": "quanta"},
            "semble-lexical-file": {"capture_id": "semble"},
        },
        "results": [{"task_id": "q1", "candidates": [{"path": "gold.go"}]}],
    }
    report_sha = "a" * 64
    report = {
        "status": "diagnostic_unqualified",
        "suite_commitment_sha256": fresh.canonical_sha(suite),
        "query_pack_sha256": fresh.canonical_sha(pack),
        "runner_record_sha256": fresh.canonical_sha(merged),
        "route_provenance": merged["route_provenance"],
    }
    verdict = {
        "comparisons": [
            {
                "report_digest": report_sha,
                "record_digest": report["runner_record_sha256"],
                "candidate_route": "lexical",
                "baseline_route": "semble-lexical-file",
            }
        ]
    }
    fresh._require_native_report_binding(report, verdict, suite, pack, merged, report_sha, "r")
    changed = copy.deepcopy(merged)
    changed["results"][0]["candidates"][0]["path"] = "wrong.go"
    with pytest.raises(fresh.FreshJoinError, match="do not bind"):
        fresh._require_native_report_binding(report, verdict, suite, pack, changed, report_sha, "r")
    changed_verdict = copy.deepcopy(verdict)
    changed_verdict["comparisons"][0]["report_digest"] = "b" * 64
    with pytest.raises(fresh.FreshJoinError, match="do not bind"):
        fresh._require_native_report_binding(
            report, changed_verdict, suite, pack, merged, report_sha, "r"
        )


def test_fresh_join_selects_successful_retry_without_hiding_failed_attempts(tmp_path):
    cell = {
        "cell_id": "l",
        "repository": "chartjs",
        "output_root": str(tmp_path / "l"),
        "spec_sha256": "new-spec",
    }
    prepared = {
        "source_commit": "source",
        "runner_sha256": "runner",
        "searchd_sha256": "searchd",
    }
    failure = {
        **prepared,
        "returncode": 2,
        "spec_sha256": "old-spec",
        "output_root": str(tmp_path / "out/l"),
    }
    success = {
        **prepared,
        "returncode": 0,
        "spec_sha256": "new-spec",
        "output_root": str(tmp_path / "l"),
    }
    (tmp_path / "l.status.json").write_text(json.dumps(failure))
    (tmp_path / "l-attempt-2.status.json").write_text(json.dumps(success))
    bound = fresh._select_native_status(cell, prepared)
    assert bound["successful_attempt_path"] == str(tmp_path / "l-attempt-2.status.json")
    assert [row["path"] for row in bound["other_attempts"]] == [str(tmp_path / "l.status.json")]
    (tmp_path / "l-attempt-3.status.json").write_text(json.dumps(success))
    with pytest.raises(fresh.FreshJoinError, match="absent or ambiguous"):
        fresh._select_native_status(cell, prepared)


def test_fresh_join_global_status_binds_driver_binary_build_and_postrun(tmp_path):
    cell = {
        "cell_id": "d", "repository": "attrs", "tasks": 2,
        "output_root": str(tmp_path / "d"), "spec_sha256": "spec",
    }
    prepared = {
        "source_commit": "driver", "driver_source_sha": "driver",
        "binary_build_source_sha": "binary-build", "driver_python_sha256": "python",
        "runner_sha256": "runner", "searchd_sha256": "searchd",
    }
    status = {
        "returncode": 0, "source_commit": "driver", "driver_source_sha": "driver",
        "binary_build_source_sha": "binary-build", "driver_python_sha256": "python",
        "runner_sha256": "runner", "searchd_sha256": "searchd",
        "spec_sha256": "spec", "output_root": str(tmp_path / "d"),
        "postrun_verification": {"pair_valid": True, "selected": 4},
    }
    path = tmp_path / "d.status.json"
    path.write_text(json.dumps(status))
    assert fresh._select_native_status(cell, prepared)["successful_attempt_path"] == str(path)
    for changed in (
        {"binary_build_source_sha": "wrong"},
        {"driver_python_sha256": "wrong"},
        {"postrun_verification": {"pair_valid": False, "selected": 4}},
        {"postrun_verification": {"pair_valid": True, "selected": 3}},
    ):
        path.write_text(json.dumps({**status, **changed}))
        with pytest.raises(fresh.FreshJoinError, match="absent or ambiguous"):
            fresh._select_native_status(cell, prepared)


def test_fresh_join_global_matrix_adapts_to_same_admission_shape(tmp_path):
    def write(path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value, sort_keys=True))
        return path

    rows, external_cells, cohorts, sources = [], [], {}, {}
    for index in range(12):
        repo = f"repo{index}"
        spec_path = write(tmp_path / repo / "spec.json", {
            "suite": str(write(tmp_path / repo / "suite.json", {
                "repository_commit": "commit", "tasks": [{"task_id": repo + ".q1"}],
            })),
        })
        identity = write(tmp_path / "gold-v10" / repo / "identity.json", {"repo": repo})
        rows.append({
            "repository": repo, "intent": fresh.INTENT,
            "status": "diagnostic_unqualified", "selected_task_ids": [repo + ".q1"],
            "repository_commit": "commit", "gold_capsule_identity_sha256": fresh.sha(identity),
            "release_digest": "release",
        })
        external_cells.append({
            "repository": repo, "spec_path": str(spec_path), "tasks": 1,
            "projected_suite_commitment_sha256": repo + "-suite",
            "blind_pack_commitment_sha256": repo + "-pack",
        })
        cohorts[repo] = {
            "selected_task_ids": [repo + ".q1"], "selected": 1,
            "projected_suite_sha256": repo + "-suite",
            "blind_pack_sha256": repo + "-pack",
        }
        sources[repo] = {"identity_sha256": fresh.sha(identity)}
    gold_path = write(tmp_path / "gold.json", {
        "status": "captured", "capsule_count": 12, "source_head": "driver",
        "dependency_versions": {"parser": "pinned"},
        "python_executable": "/pinned/python",
        "pyproject_sha256": "project", "uv_lock_sha256": "lock",
        "identity_sha256": {repo: row["identity_sha256"] for repo, row in sources.items()},
    })
    matrix_path = write(tmp_path / "admission-matrix.json", {
        "schema_version": 1, "status": "diagnostic_unqualified",
        "repository_count": 12, "intent_count": 6, "release_digest": "release",
        "cells": rows,
    })
    matrix_receipt_path = write(tmp_path / "matrix-receipt.json", {
        "status": "factory_derived_diagnostic_unqualified",
        "matrix_sha256": fresh.sha(matrix_path),
        "gold_receipt_sha256": fresh.sha(gold_path),
    })
    projection_path = write(tmp_path / "ordinary-receipt.json", {
        "status": "global_ordinary_diagnostic_unqualified",
        "source_matrix_sha256": fresh.sha(matrix_path),
        "repository_count": 12, "selected_total": 12,
        "source_base_head": "driver", "projector_source_commit": "projector",
        "projector_overlay_sha256": "overlay", "cohorts": cohorts,
    })
    prepared = {
        "source_commit": "driver", "driver_source_sha": "driver",
        "driver_python": "/pinned/python",
        "parser_dependency_versions": {"parser": "pinned"},
        "pyproject_sha256": "project", "uv_lock_sha256": "lock",
        "gold_receipt_sha256": fresh.sha(gold_path),
        "projection_receipt_sha256": fresh.sha(projection_path),
        "projector_overlay_sha256": "overlay", "projector_source_commit": "projector",
        "binary_build_source_sha": "binary", "driver_python_sha256": "python",
    }
    manifest = {
        "schema": "c5_osa1_external_global12_fresh_v1",
        "release_digest": "release", "matrix_path": str(matrix_path),
        "matrix_sha256": fresh.sha(matrix_path),
        "matrix_receipt_path": str(matrix_receipt_path),
        "ordinary_projection_receipt_path": str(projection_path),
        "ordinary_projection_receipt_sha256": fresh.sha(projection_path),
        "gold_receipt_path": str(gold_path),
        "gold_producer_runtime": {
            "receipt_sha256": fresh.sha(gold_path), "source_head": "driver",
            "dependency_versions": {"parser": "pinned"},
            "python_executable": "/pinned/python", "python_executable_sha256": "python",
            "pyproject_sha256": "project", "uv_lock_sha256": "lock",
        },
        "suite_projector_source": {
            "base_head": "driver", "commit": "projector", "overlay_sha256": "overlay",
        },
        "gold_producer_sources": sources,
        "cells": external_cells, "total_tasks": 12,
    }
    admission, custody = fresh._source_admission(matrix_path, prepared, manifest)
    assert len(admission) == 12 and all(row["status"] == "VALID" for row in admission)
    assert custody["cohort_contract"] == "c5_global_c4_ordinary_osa1_v1"
    changed = copy.deepcopy(manifest)
    changed["cells"][0]["blind_pack_commitment_sha256"] = "wrong"
    with pytest.raises(fresh.FreshJoinError, match="global source admission task"):
        fresh._source_admission(matrix_path, prepared, changed)
    changed = {**prepared, "binary_build_source_sha": ""}
    with pytest.raises(fresh.FreshJoinError, match="global gold, matrix, projection"):
        fresh._source_admission(matrix_path, changed, manifest)
    old, old_custody = fresh._source_admission(
        write(tmp_path / "old-audit.json", [{"repository": "repo", "status": "VALID"}]),
        {}, {},
    )
    assert old == [{"repository": "repo", "status": "VALID"}]
    assert old_custody["cohort_contract"] == "c5_fixed_original_source_eligibility_v1"


def test_fresh_join_refuses_cross_pair_producer_or_evaluator_drift():
    original = {field: field + "-frozen" for field in fresh.PAIR_CUSTODY_FIELDS}
    fresh._require_pair_custody_consistency([original, copy.deepcopy(original)])
    for field in fresh.PAIR_CUSTODY_FIELDS:
        changed = {**original, field: field + "-new"}
        with pytest.raises(fresh.FreshJoinError, match="different source, binary"):
            fresh._require_pair_custody_consistency([original, changed])
    with pytest.raises(fresh.FreshJoinError, match="custody incomplete"):
        fresh._require_pair_custody_consistency([{**original, "native_runner_sha256": None}])


def test_fresh_join_refuses_dirty_producer_at_unchanged_head(tmp_path, monkeypatch):
    owner = tmp_path / "owner.py"
    owner.write_text("frozen = True\n")
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    subprocess.run(["git", "-C", str(tmp_path), "add", "owner.py"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(tmp_path),
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "freeze",
        ],
        check=True,
    )
    head = (
        subprocess.check_output(["git", "-C", str(tmp_path), "rev-parse", "HEAD"]).decode().strip()
    )
    monkeypatch.setattr(fresh, "EXTERNAL_PRODUCER_FILES", {"producer": "owner.py"})
    assert fresh._external_producer_sources(tmp_path, head) == {"producer": fresh.sha(owner)}
    owner.write_text("frozen = False\n")
    with pytest.raises(fresh.FreshJoinError, match="differs from frozen HEAD"):
        fresh._external_producer_sources(tmp_path, head)
