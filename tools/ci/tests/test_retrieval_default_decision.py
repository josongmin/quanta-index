"""Product-default decisions stay separate from qualified evidence validity."""

from __future__ import annotations

import hashlib
import json

import pytest

from tools.benchmark.retrieval import decision

PROOF = "a" * 64


def fixture_inputs(delta: float = 0.07) -> tuple[dict, dict, dict, dict, dict]:
    policy = {
        "schema_version": 1,
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
        "comparisons": [{**policy["comparison"], "primary_delta": delta, "report_digest": PROOF}],
    }
    report = {
        "rank_metrics": {
            "comparison": {
                "baseline": "semble-hybrid",
                "candidate": "hybrid",
                "primary_metric": "ndcg_at_10",
                "primary_delta": delta,
                "stratified_primary_delta": {
                    "category": {"definition": {"sample_count": 25, "mean_delta": 0.04}}
                },
                "no_answer_abstention_delta": {"sample_count": 5, "mean_delta": 0.0},
            }
        }
    }
    cluster = {
        "method": "paired_query_family_cluster_bootstrap_percentile_v1",
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


def test_capture_binding_selects_report_and_owned_resource_root(monkeypatch, tmp_path) -> None:
    policy, verdict, report, cluster, _measurements = fixture_inputs()
    policy_path = tmp_path / "policy.json"
    policy_path.write_text(json.dumps(policy), encoding="utf-8")
    policy_sha = hashlib.sha256(policy_path.read_bytes()).hexdigest()
    admission = tmp_path / "admission.json"
    admission.write_text(json.dumps({"decision_policy_sha256": policy_sha}), encoding="utf-8")
    suite = tmp_path / "suite.json"
    suite.write_text("{}", encoding="utf-8")
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
