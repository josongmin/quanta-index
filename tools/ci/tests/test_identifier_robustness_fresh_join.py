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
