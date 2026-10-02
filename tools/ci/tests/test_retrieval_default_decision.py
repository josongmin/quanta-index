"""Product-default decisions stay separate from qualified evidence validity."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from tools.benchmark.retrieval import decision
from tools.benchmark.retrieval import evaluator as ev

PROOF = "a" * 64
REPOSITORY = "b" * 40


def repository_disjoint_policy(tmp_path: Path, split_sha: str) -> dict:
    rows = [
        {
            "repository": f"repo-{index:02d}",
            "repository_commit": f"{index + 1:040x}",
            "release_digest": "sha256:" + "a" * 64,
            "stratum": f"language-{index // 3}",
            "suite_sha256": "b" * 64,
            "query_family_ids": [f"repo-{index:02d}.family"],
            "categories": ["objective"],
        }
        for index in range(12)
    ]
    return {
        "schema_version": 2,
        "repository_scope": {
            "kind": "repository_disjoint_holdout",
            "split_manifest_sha256": split_sha,
            "releases": {"sha256:" + "a" * 64: str(tmp_path / "release")},
            "holdout": rows,
        },
        "comparison": {
            "strategy": "whole_file",
            "baseline_route": "semble-hybrid",
            "candidate_route": "hybrid",
            "primary_metric": "ndcg_at_10",
        },
        "min_useful_delta": 0.05,
        "min_cluster_lower_95": 0.0,
        "confidence_method": "paired_stratified_repository_cluster_bootstrap_percentile_v1",
        "critical_strata": [
            {"axis": "no_answer", "name": "all", "min_delta": 0.0},
            *(
                {"axis": "repository", "name": row["repository"], "min_delta": -0.01}
                for row in rows
            ),
        ],
        "resource_limits": {
            "max_query_p95_ms": 100.0,
            "max_peak_rss_bytes": 1_000_000,
            "max_index_bytes": 2_000_000,
        },
    }


def test_repository_disjoint_policy_requires_full_frozen_roster(tmp_path):
    policy = repository_disjoint_policy(tmp_path, "c" * 64)
    assert decision.validate_repository_disjoint_policy(policy) == policy
    policy["repository_scope"]["holdout"].pop()
    with pytest.raises(decision.DecisionError, match="at least twelve"):
        decision.validate_repository_disjoint_policy(policy)
    policy = repository_disjoint_policy(tmp_path, "c" * 64)
    policy["critical_strata"].pop()
    with pytest.raises(decision.DecisionError, match="omit a holdout repository"):
        decision.validate_repository_disjoint_policy(policy)
    policy = repository_disjoint_policy(tmp_path, "c" * 64)
    policy["repository_scope"]["holdout"][-1]["stratum"] = "singleton"
    with pytest.raises(decision.DecisionError, match="strata are invalid"):
        decision.validate_repository_disjoint_policy(policy)


def test_repository_disjoint_bundle_replays_policy_bound_captures(monkeypatch, tmp_path):
    split_path = tmp_path / "split.json"
    split_path.write_text("{}")
    split_sha = hashlib.sha256(split_path.read_bytes()).hexdigest()
    policy = repository_disjoint_policy(tmp_path, split_sha)
    captures = []
    for row in policy["repository_scope"]["holdout"]:
        name = row["repository"]
        root = tmp_path / name
        root.mkdir()
        suite = {
            "suite_id": name,
            "repository_commit": row["repository_commit"],
            "file_universe_digest": "sha256:" + "d" * 64,
            "tasks": [
                {
                    "task_id": "T1",
                    "split": "eval",
                    "gold": [1],
                    "query_family_id": row["query_family_ids"][0],
                    "category": "objective",
                }
            ],
        }
        suite_path = root / "suite.json"
        suite_path.write_bytes(ev.canonical(suite))
        row["suite_sha256"] = hashlib.sha256(suite_path.read_bytes()).hexdigest()
        report = {
            "suite_id": name,
            "suite_commitment_sha256": ev.digest(ev.canonical(suite)),
            "repository_commit": row["repository_commit"],
            "graded": True,
            "rank_metrics": {
                "comparison": {
                    "baseline": "semble-hybrid",
                    "candidate": "hybrid",
                    "primary_metric": "ndcg_at_10",
                    "sample_count": 1,
                    "primary_delta": 0.5,
                }
            },
            "per_query": [
                {"task_id": "T1", "route": "semble-hybrid", "ndcg_at_10": 0.25},
                {"task_id": "T1", "route": "hybrid", "ndcg_at_10": 0.75},
            ],
        }
        (root / "report.json").write_bytes(ev.canonical(report))
        (root / "manifest.json").write_text(
            json.dumps(
                {
                    "scope": "qualified",
                    "artifacts": {
                        "admission_manifest": "admission.json",
                        "reports": ["report.json"],
                    },
                }
            )
        )
        captures.append(
            {
                "repository": name,
                "checkout": str(root),
                "suite": str(suite_path),
                "run_manifest": str(root / "manifest.json"),
            }
        )
    policy_path = tmp_path / "policy.json"
    policy_path.write_bytes(ev.canonical(policy))
    policy_sha = hashlib.sha256(policy_path.read_bytes()).hexdigest()
    for row in policy["repository_scope"]["holdout"]:
        root = tmp_path / row["repository"]
        (root / "admission.json").write_text(
            json.dumps(
                {
                    "decision_policy_sha256": policy_sha,
                    "repository_commit": row["repository_commit"],
                    "suite_sha256": row["suite_sha256"],
                }
            )
        )
    bundle_path = tmp_path / "bundle.json"
    bundle_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "repository_disjoint_c5_inputs",
                "policy": str(policy_path),
                "split_manifest": str(split_path),
                "captures": captures,
            }
        )
    )
    monkeypatch.setattr(
        decision.corpus_binding,
        "validate_split_manifest",
        lambda _raw, _releases: {
            "repositories": [
                {
                    "split": "holdout",
                    "repository": row["repository"],
                    "repository_commit": row["repository_commit"],
                    "release_digest": row["release_digest"],
                    "query_family_ids": row["query_family_ids"],
                    "code_only_universe_digest": "sha256:" + "d" * 64,
                }
                for row in policy["repository_scope"]["holdout"]
            ]
        },
    )
    monkeypatch.setattr(decision.run, "_validate_manifest_shape", lambda value: value)
    monkeypatch.setattr(decision.run, "_resolve_artifact", lambda root, ref, _role: root / ref)
    monkeypatch.setattr(decision.run, "validate_admission_manifest", lambda value: value)

    def verdict(_checkout, _suite, manifest_path):
        report_path = manifest_path.parent / "report.json"
        return {
            "failure_class": "none",
            "states": {
                name: "pass"
                for name in (
                    "PAIR_VALID",
                    "CONTRACT_GREEN",
                    "SDK_PATH_GREEN",
                    "QUALITY_DELTA",
                    "PERF_QUALIFIED",
                )
            },
            "comparisons": [
                {
                    **policy["comparison"],
                    "graded": True,
                    "report_digest": hashlib.sha256(report_path.read_bytes()).hexdigest(),
                }
            ],
        }

    monkeypatch.setattr(decision.run, "build_verdict", verdict)
    result = decision.replay_repository_disjoint_bundle(bundle_path)
    assert result["status"] == "replayed_no_default_decision"
    assert result["product_default_decision"] is False
    assert (result["repository_count"], result["paired_sample_count"]) == (12, 12)
    assert result["repository_cluster_ci"]["mean"] == 0.5
    (tmp_path / "repo-00" / "admission.json").write_text(
        json.dumps({"decision_policy_sha256": "0" * 64, "repository_commit": "1".zfill(40), "suite_sha256": policy["repository_scope"]["holdout"][0]["suite_sha256"]})
    )
    with pytest.raises(decision.DecisionError, match="not frozen"):
        decision.replay_repository_disjoint_bundle(bundle_path)


def fixture_inputs(delta: float = 0.07) -> tuple[dict, dict, dict, dict, dict]:
    policy = {
        "schema_version": 1,
        "repository_scope": {
            "kind": "single_repository",
            "repository_commit": REPOSITORY,
        },
        "comparison": {
            "strategy": "whole_file",
            "baseline_route": "semble-hybrid",
            "candidate_route": "hybrid",
            "primary_metric": "ndcg_at_10",
        },
        "min_useful_delta": 0.05,
        "min_cluster_lower_95": 0.0,
        "confidence_method": "paired_query_family_cluster_bootstrap_percentile_v1",
        "critical_strata": [
            {"axis": "category", "name": "definition", "min_delta": -0.01},
            {"axis": "no_answer", "name": "all", "min_delta": 0.0},
        ],
        "resource_limits": {
            "max_query_p95_ms": 100.0,
            "max_peak_rss_bytes": 1_000_000,
            "max_index_bytes": 2_000_000,
        },
    }
    states = {
        key: "pass"
        for key in (
            "PAIR_VALID",
            "CONTRACT_GREEN",
            "SDK_PATH_GREEN",
            "QUALITY_DELTA",
            "PERF_QUALIFIED",
        )
    }
    verdict = {
        "states": states,
        "state_evidence": {key: {"proof_digest": PROOF} for key in states},
        "failure_class": "none",
        "comparisons": [
            {
                **policy["comparison"],
                "primary_delta": delta,
                "report_digest": PROOF,
                "graded": True,
                "sample_count": 25,
            }
        ],
    }
    report = {
        "repository_commit": REPOSITORY,
        "graded": True,
        "rank_metrics": {
            "comparison": {
                "baseline": "semble-hybrid",
                "candidate": "hybrid",
                "primary_metric": "ndcg_at_10",
                "primary_delta": delta,
                "sample_count": 25,
                "stratified_primary_delta": {
                    "category": {"definition": {"sample_count": 25, "mean_delta": 0.04}}
                },
                "no_answer_abstention_delta": {"sample_count": 5, "mean_delta": 0.0},
            }
        },
    }
    cluster = {
        "method": "paired_query_family_cluster_bootstrap_percentile_v1",
        "sample_count": 25,
        "cluster_count": 25,
        "lower_95": 0.01,
    }
    measurements = {"query_p95_ms": 90.0, "peak_rss_bytes": 900_000, "index_bytes": 1_900_000}
    return policy, verdict, report, cluster, measurements


def test_admitted_evidence_still_refuses_negative_and_zero_primary_delta() -> None:
    for delta in (-0.02, 0.0):
        policy, verdict, report, cluster, measurements = fixture_inputs(delta)
        cluster["lower_95"] = delta - 0.01
        result = decision.evaluate_decision(policy, verdict, report, cluster, measurements)
        assert result["status"] == "refuse"
        assert "primary_effect_below_minimum" in result["reasons"]


def test_effect_stratum_and_resource_budgets_are_independent() -> None:
    policy, verdict, report, cluster, measurements = fixture_inputs()
    assert (
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)["status"]
        == "admit"
    )
    cluster["lower_95"] = -0.001
    report["rank_metrics"]["comparison"]["stratified_primary_delta"]["category"]["definition"][
        "mean_delta"
    ] = -0.02
    measurements.update(query_p95_ms=101.0, peak_rss_bytes=1_000_001, index_bytes=2_000_001)
    result = decision.evaluate_decision(policy, verdict, report, cluster, measurements)
    assert result["status"] == "refuse"
    assert set(result["reasons"]) == {
        "cluster_lower_bound_below_minimum",
        "critical_stratum_regression:category:definition",
        "query_p95_budget_exceeded",
        "peak_rss_budget_exceeded",
        "index_budget_exceeded",
    }


def test_missing_predeclared_or_observed_input_fails_closed() -> None:
    policy, verdict, report, cluster, measurements = fixture_inputs()
    for invalid in (0, -0.01, None, float("nan"), True):
        broken = {**policy, "min_useful_delta": invalid}
        with pytest.raises(decision.DecisionError):
            decision.evaluate_decision(broken, verdict, report, cluster, measurements)
    del report["rank_metrics"]["comparison"]["stratified_primary_delta"]["category"]["definition"]
    with pytest.raises(decision.DecisionError, match="critical stratum"):
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)
    report = fixture_inputs()[2]
    verdict["states"]["PERF_QUALIFIED"] = "not_applicable"
    with pytest.raises(decision.DecisionError, match="PERF_QUALIFIED"):
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)


def test_repository_scope_is_bound_and_multi_repository_cannot_use_single_repo_ci() -> None:
    policy, verdict, report, cluster, measurements = fixture_inputs()
    for changed in (
        {"kind": "multi_repository", "repository_commit": REPOSITORY},
        {"kind": "single_repository", "repository_commit": "c" * 40},
        {"kind": "single_repository", "repository_commit": "short"},
    ):
        altered = {**policy, "repository_scope": changed}
        with pytest.raises(decision.DecisionError, match="repository"):
            decision.evaluate_decision(altered, verdict, report, cluster, measurements)
    assert (
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)[
            "repository_scope"
        ]
        == policy["repository_scope"]
    )
    report["repository_commit"] = "c" * 40
    with pytest.raises(decision.DecisionError, match="bound report repository"):
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)


def test_unsupported_comparator_and_missing_coverage_refuse() -> None:
    policy, verdict, report, cluster, measurements = fixture_inputs()
    verdict["comparisons"][0]["graded"] = False
    with pytest.raises(decision.DecisionError, match="not graded"):
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)
    verdict["comparisons"][0]["graded"] = True
    report["graded"] = False
    with pytest.raises(decision.DecisionError, match="bound report is not graded"):
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)
    report["graded"] = True
    report["rank_metrics"]["comparison"]["no_answer_abstention_delta"]["sample_count"] = 0
    with pytest.raises(decision.DecisionError, match="lacks coverage"):
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)
    report["rank_metrics"]["comparison"]["no_answer_abstention_delta"]["sample_count"] = 5
    verdict["comparisons"][0]["sample_count"] = 24
    with pytest.raises(decision.DecisionError, match="paired graded coverage"):
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)
    verdict["comparisons"][0]["sample_count"] = 25
    cluster["sample_count"] = 24
    with pytest.raises(decision.DecisionError, match="cluster interval lacks paired coverage"):
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)


def test_no_answer_stratum_must_be_predeclared() -> None:
    policy, verdict, report, cluster, measurements = fixture_inputs()
    policy["critical_strata"].pop()
    with pytest.raises(decision.DecisionError, match="include no_answer"):
        decision.evaluate_decision(policy, verdict, report, cluster, measurements)


def test_policy_must_be_frozen_in_admission_before_replay(monkeypatch, tmp_path) -> None:
    policy = tmp_path / "policy.json"
    policy.write_text(json.dumps(fixture_inputs()[0]), encoding="utf-8")
    manifest = tmp_path / "run-manifest.json"
    manifest.write_text("{}", encoding="utf-8")
    admission = tmp_path / "admission.json"
    admission.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(
        decision.run,
        "_validate_manifest_shape",
        lambda _value: {
            "scope": "qualified",
            "artifacts": {"admission_manifest": "admission.json"},
        },
    )
    monkeypatch.setattr(decision.run, "validate_admission_manifest", lambda _value: {})
    monkeypatch.setattr(
        decision.run,
        "build_verdict",
        lambda *_args: pytest.fail("replay must not begin without a frozen policy"),
    )
    with pytest.raises(decision.DecisionError, match="not frozen"):
        decision.build_decision(tmp_path, tmp_path / "suite.json", manifest, policy)


def test_policy_rejects_duplicate_keys_before_replay(monkeypatch, tmp_path) -> None:
    policy = tmp_path / "policy.json"
    policy.write_text('{"schema_version": 1, "schema_version": 1}', encoding="utf-8")
    manifest = tmp_path / "run-manifest.json"
    manifest.write_text("{}", encoding="utf-8")
    admission = tmp_path / "admission.json"
    admission.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(
        decision.run,
        "_validate_manifest_shape",
        lambda _value: {
            "scope": "qualified",
            "artifacts": {"admission_manifest": "admission.json"},
        },
    )
    monkeypatch.setattr(decision.run, "validate_admission_manifest", lambda _value: {})
    with pytest.raises(ValueError, match="duplicate JSON key"):
        decision.build_decision(tmp_path, tmp_path / "suite.json", manifest, policy)


def test_capture_binding_selects_report_and_owned_resource_root(monkeypatch, tmp_path) -> None:
    policy, verdict, report, cluster, _measurements = fixture_inputs()
    policy_path = tmp_path / "policy.json"
    policy_path.write_text(json.dumps(policy), encoding="utf-8")
    policy_sha = hashlib.sha256(policy_path.read_bytes()).hexdigest()
    admission = tmp_path / "admission.json"
    admission.write_text(json.dumps({"decision_policy_sha256": policy_sha}), encoding="utf-8")
    suite = tmp_path / "suite.json"
    suite.write_text(json.dumps({"repository_commit": REPOSITORY}), encoding="utf-8")
    manifest = tmp_path / "run-manifest.json"
    manifest.write_text("{}", encoding="utf-8")
    report_path = tmp_path / "report.json"
    report_path.write_text(json.dumps(report), encoding="utf-8")
    record = tmp_path / "record.json"
    record.write_text(
        json.dumps({"captures": {"hybrid": {"system": "quanta", "chunk_strategy": "whole_file"}}}),
        encoding="utf-8",
    )
    resource = tmp_path / "resource.json"
    resource.write_text(
        json.dumps(
            {
                "subject_sha256": decision.run.sha_file(record),
                "peak_rss_bytes": 900_000,
                "storage": {"index_bytes": 1_900_000},
            }
        ),
        encoding="utf-8",
    )
    matrix = tmp_path / "matrix.json"
    matrix.write_text(
        json.dumps(
            {
                "samples": {"quanta:whole_file:hybrid:task-1": [88.0, 90.0]},
                "floors": {"quanta:whole_file:hybrid": 2},
            }
        ),
        encoding="utf-8",
    )
    verdict["comparisons"][0]["report_digest"] = decision.run.sha_file(report_path)
    artifacts = {
        "admission_manifest": admission.name,
        "latency_matrix": matrix.name,
        "reports": [report_path.name],
        "records": [record.name],
        "resource_metrics": [resource.name],
    }
    monkeypatch.setattr(
        decision.run,
        "_validate_manifest_shape",
        lambda _value: {"scope": "qualified", "artifacts": artifacts, "repetitions": 1},
    )
    monkeypatch.setattr(decision.run, "validate_admission_manifest", lambda value: value)
    monkeypatch.setattr(decision.run, "build_verdict", lambda *_args: verdict)
    monkeypatch.setattr(decision, "qualified_query_family_ci", lambda *_args: cluster)
    result = decision.build_decision(tmp_path, suite, manifest, policy_path)
    assert result["status"] == "admit"
    assert result["policy_sha256"] == policy_sha
    assert result["report_digest"] == decision.run.sha_file(report_path)
    suite.write_text(json.dumps({"repository_commit": "c" * 40}), encoding="utf-8")
    with pytest.raises(decision.DecisionError, match="bound suite repository"):
        decision.build_decision(tmp_path, suite, manifest, policy_path)
    suite.write_text(json.dumps({"repository_commit": REPOSITORY}), encoding="utf-8")
    matrix.write_text(
        json.dumps(
            {
                "samples": {"quanta:whole_file:hybrid:task-1": [88.0, 90.0]},
                "floors": {"quanta:whole_file:hybrid": 3},
            }
        ),
        encoding="utf-8",
    )
    with pytest.raises(decision.DecisionError, match="floor or samples"):
        decision.build_decision(tmp_path, suite, manifest, policy_path)

    # Mutation after parsing but before collecting the artifact inventory must
    # not replace the hash of the manifest whose references were consumed.
    matrix.write_text(
        json.dumps(
            {
                "samples": {"quanta:whole_file:hybrid:task-1": [88.0, 90.0]},
                "floors": {"quanta:whole_file:hybrid": 2},
            }
        ),
        encoding="utf-8",
    )
    resolve_artifact = decision.run._resolve_artifact

    def mutate_manifest(root, ref, label):
        if label == "admission":
            manifest.write_text('{"substituted": true}', encoding="utf-8")
        return resolve_artifact(root, ref, label)

    monkeypatch.setattr(decision.run, "_resolve_artifact", mutate_manifest)
    with pytest.raises(decision.DecisionError, match="inputs changed"):
        decision.build_decision(tmp_path, suite, manifest, policy_path)
