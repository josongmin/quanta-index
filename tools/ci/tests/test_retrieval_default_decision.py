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
            "query_family_ids": [f"repo-{index:02d}.objective", f"repo-{index:02d}.reviewed"],
            "categories": ["objective", "reviewed"],
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
        "track_min_delta": {"objective": 0.0, "reviewed": 0.0},
        "confidence_method": "paired_stratified_repository_cluster_bootstrap_percentile_v1",
        "critical_strata": [
            {"axis": "no_answer", "name": "all", "min_delta": 0.0},
            {"axis": "category", "name": "objective", "min_delta": 0.0},
            {"axis": "category", "name": "reviewed", "min_delta": 0.0},
            {"axis": "language", "name": "go", "min_delta": 0.0},
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
    policy = repository_disjoint_policy(tmp_path, "c" * 64)
    del policy["track_min_delta"]["reviewed"]
    with pytest.raises(decision.DecisionError, match="repository-disjoint tracks"):
        decision.validate_repository_disjoint_policy(policy)


@pytest.mark.parametrize("file_policy", [False, True])
def test_repository_disjoint_bundle_replays_policy_bound_captures(
    monkeypatch, tmp_path, file_policy
):
    split_path = tmp_path / "split.json"
    split_path.write_text("{}")
    split_sha = hashlib.sha256(split_path.read_bytes()).hexdigest()
    policy = repository_disjoint_policy(tmp_path, split_sha)
    if file_policy:
        policy["schema_version"] = 3
        policy["metric_scope"] = "scored_distinct_file"
        policy["request_mode"] = "default_file_search"
        policy["comparison"]["primary_metric"] = "file_ndcg_at_10"
    captures = []
    for row in policy["repository_scope"]["holdout"]:
        name = row["repository"]
        root = tmp_path / name
        root.mkdir()
        suite = {
            "suite_id": name,
            "repository_commit": row["repository_commit"],
            "file_universe_digest": "d" * 64,
            "tasks": [
                {
                    "task_id": "T1",
                    "split": "eval",
                    "gold": [{"path": "file.go"}],
                    "query_family_id": row["query_family_ids"][0],
                    "category": "objective",
                    "judgment_policy": ev.SOURCE_ORACLE_JUDGMENT_POLICY,
                    "source_oracle": {},
                },
                {
                    "task_id": "T2",
                    "split": "eval",
                    "gold": [],
                    "query_family_id": row["query_family_ids"][0],
                    "category": "objective",
                },
                {
                    "task_id": "T3",
                    "split": "eval",
                    "gold": [{"path": "file.go"}],
                    "query_family_id": row["query_family_ids"][1],
                    "category": "reviewed",
                    "judgment_policy": ev.COMPLETE_JUDGMENT_POLICY,
                    "file_judgments": [{"path": "file.go", "relevance": 1}],
                    "label_review": {"assessment": "reviewed_unambiguous"},
                },
            ],
        }
        if file_policy:
            for task in suite["tasks"]:
                task["evaluation_contract"] = {
                    "request_mode": "default_file_search",
                    "gold_unit": "distinct_file",
                    "result_unit": "distinct_file",
                }
                task["file_judgments"] = task.get("file_judgments", [])
                if "source_oracle" in task:
                    task["source_oracle"]["unit"] = "distinct_file"
        suite_path = root / "suite.json"
        suite_path.write_bytes(ev.canonical(suite))
        row["suite_sha256"] = hashlib.sha256(suite_path.read_bytes()).hexdigest()
        report = {
            "suite_id": name,
            "suite_commitment_sha256": ev.digest(ev.canonical(suite)),
            "repository_commit": row["repository_commit"],
            "graded": True,
            "rank_metric_version": (
                "file-judgments-complete-v1"
                if file_policy
                else "rb-rank-context-density-first-coverage"
            ),
            **(
                {
                    "report_scope": "paired_complete_scored_file_evidence_v1",
                    "status": "evidence_unqualified",
                }
                if file_policy
                else {}
            ),
            "rank_metrics": {
                "comparison": {
                    "baseline": "semble-hybrid",
                    "candidate": "hybrid",
                    "primary_metric": policy["comparison"]["primary_metric"],
                    "sample_count": 2,
                    "primary_delta": 0.5,
                    "no_answer_abstention_delta": {"sample_count": 1, "mean_delta": 0.0},
                }
            },
            "per_query": [
                {"task_id": "T1", "route": "semble-hybrid", "file_ndcg_at_10" if file_policy else "ndcg_at_10": 0.25},
                {"task_id": "T1", "route": "hybrid", "file_ndcg_at_10" if file_policy else "ndcg_at_10": 0.75},
                {"task_id": "T2", "route": "semble-hybrid", "status": "ok"},
                {"task_id": "T2", "route": "hybrid", "status": "ok"},
                {"task_id": "T3", "route": "semble-hybrid", "file_ndcg_at_10" if file_policy else "ndcg_at_10": 0.25},
                {"task_id": "T3", "route": "hybrid", "file_ndcg_at_10" if file_policy else "ndcg_at_10": 0.75},
            ],
        }
        (root / "report.json").write_bytes(ev.canonical(report))
        record_path = root / "record.json"
        record_path.write_text(json.dumps({"repository": name}))
        (root / "resource.json").write_text(
            json.dumps(
                {
                    "subject_sha256": hashlib.sha256(record_path.read_bytes()).hexdigest(),
                    "peak_rss_bytes": 1_000,
                    "storage": {"index_bytes": 2_000},
                }
            )
        )
        (root / "latency.json").write_text(
            json.dumps(
                {
                    "samples": {
                        "quanta:whole_file:hybrid:T1": [5.0],
                        "quanta:whole_file:hybrid:T3": [5.0],
                    },
                    "floors": {"quanta:whole_file:hybrid": 2},
                }
            )
        )
        (root / "manifest.json").write_text(
            json.dumps(
                {
                    "scope": "qualified",
                    "repetitions": 1,
                    "artifacts": {
                        "admission_manifest": "admission.json",
                        "reports": ["report.json"],
                        "records": ["record.json"],
                        "resource_metrics": ["resource.json"],
                        "latency_matrix": "latency.json",
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
                    "schema_version": 3,
                    "decision_policy_sha256": policy_sha,
                    "repository_commit": row["repository_commit"],
                    "suite_sha256": row["suite_sha256"],
                    "repository_disjoint": {
                        "repository": row["repository"],
                        "release_digest": row["release_digest"],
                        "split_manifest_sha256": split_sha,
                    },
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
    split_rows = [
        {
            "split": "holdout",
            "repository": row["repository"],
            "repository_commit": row["repository_commit"],
            "release_digest": row["release_digest"],
            "query_family_ids": row["query_family_ids"].copy(),
            "code_only_universe_digest": "sha256:" + "d" * 64,
        }
        for row in policy["repository_scope"]["holdout"]
    ]
    monkeypatch.setattr(
        decision, "_validate_split_manifest", lambda _raw, _releases: {"repositories": split_rows}
    )
    monkeypatch.setattr(decision.run, "_validate_manifest_shape", lambda value: value)
    monkeypatch.setattr(decision.run, "_resolve_artifact", lambda root, ref, _role: root / ref)
    monkeypatch.setattr(decision.run, "validate_admission_manifest", lambda value: value)
    monkeypatch.setattr(
        decision.run, "_record_identity", lambda _record, _path: ("quanta", "whole_file")
    )

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
            "state_evidence": {
                name: {"proof_digest": "f" * 64}
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
                    "sample_count": 2,
                    "primary_delta": 0.5,
                    "report_digest": hashlib.sha256(report_path.read_bytes()).hexdigest(),
                }
            ],
        }

    monkeypatch.setattr(decision.run, "build_verdict", verdict)
    result = decision.replay_repository_disjoint_bundle(bundle_path)
    assert result["status"] == "replayed_no_default_decision"
    assert result["product_default_decision"] is False
    assert result["metric_scope"] == (
        "scored_distinct_file" if file_policy else "context_span_density"
    )
    assert (result["repository_count"], result["paired_sample_count"]) == (12, 24)
    assert result["repository_cluster_ci"]["mean"] == 0.5
    assert result["metric_gate"] == {
        "status": "eligible_for_human_review",
        "reasons": [],
        "primary_delta": 0.5,
        "repository_lower_95": 0.5,
        "measurements": {
            "query_p95_ms": 5.0,
            "peak_rss_bytes": 1_000,
            "index_bytes": 2_000,
        },
        "tracks": {
            "objective": {"repository_count": 12, "mean_delta": 0.5},
            "reviewed": {"repository_count": 12, "mean_delta": 0.5},
        },
    }
    names = [row["repository"] for row in policy["repository_scope"]["holdout"]]
    suites = {name: json.loads((tmp_path / name / "suite.json").read_text()) for name in names}
    reports = {name: json.loads((tmp_path / name / "report.json").read_text()) for name in names}
    policy["min_useful_delta"] = 0.6
    assert decision._repository_disjoint_metric_gate(
        policy,
        suites,
        reports,
        result["repository_cluster_ci"],
        result["captures"],
    )["reasons"] == ["primary_effect_below_minimum"]
    policy["min_useful_delta"] = 0.05
    policy["track_min_delta"]["reviewed"] = 0.6
    assert decision._repository_disjoint_metric_gate(
        policy, suites, reports, result["repository_cluster_ci"], result["captures"]
    )["reasons"] == ["track_regression:reviewed"]
    policy["track_min_delta"]["reviewed"] = 0.0
    suites["repo-00"]["tasks"][2]["label_review"]["assessment"] = "unreviewed"
    reports["repo-00"]["suite_commitment_sha256"] = ev.digest(ev.canonical(suites["repo-00"]))
    with pytest.raises(decision.DecisionError, match="lacks objective or reviewed labels"):
        decision._repository_disjoint_metric_gate(
            policy, suites, reports, result["repository_cluster_ci"], result["captures"]
        )
    suites["repo-00"]["tasks"][2]["label_review"]["assessment"] = "reviewed_unambiguous"
    reports["repo-00"]["suite_commitment_sha256"] = ev.digest(ev.canonical(suites["repo-00"]))
    reviewed_task = suites["repo-00"]["tasks"][2]
    review = reviewed_task.pop("label_review")
    judgments = reviewed_task.pop("file_judgments")
    reviewed_task["judgment_policy"] = ev.SOURCE_ORACLE_JUDGMENT_POLICY
    reviewed_task["source_oracle"] = {}
    reports["repo-00"]["suite_commitment_sha256"] = ev.digest(ev.canonical(suites["repo-00"]))
    with pytest.raises(decision.DecisionError, match="objective or reviewed track is missing"):
        decision._repository_disjoint_metric_gate(
            policy, suites, reports, result["repository_cluster_ci"], result["captures"]
        )
    del reviewed_task["source_oracle"]
    reviewed_task["judgment_policy"] = ev.COMPLETE_JUDGMENT_POLICY
    reviewed_task["file_judgments"] = judgments
    reviewed_task["label_review"] = review
    reports["repo-00"]["suite_commitment_sha256"] = ev.digest(ev.canonical(suites["repo-00"]))
    policy["resource_limits"]["max_query_p95_ms"] = 4.0
    assert decision._repository_disjoint_metric_gate(
        policy, suites, reports, result["repository_cluster_ci"], result["captures"]
    )["reasons"] == ["query_p95_budget_exceeded"]
    policy["resource_limits"]["max_query_p95_ms"] = 100.0
    policy["critical_strata"] = [
        row for row in policy["critical_strata"] if row["axis"] != "language"
    ]
    with pytest.raises(decision.DecisionError, match="omits an observed critical stratum"):
        decision._repository_disjoint_metric_gate(
            policy, suites, reports, result["repository_cluster_ci"], result["captures"]
        )
    policy["critical_strata"].insert(2, {"axis": "language", "name": "go", "min_delta": 0.0})
    reports["repo-00"]["rank_metrics"]["comparison"]["no_answer_abstention_delta"]["mean_delta"] = (
        1.0
    )
    with pytest.raises(decision.DecisionError, match="no-answer mean differs"):
        decision._repository_disjoint_metric_gate(
            policy, suites, reports, result["repository_cluster_ci"], result["captures"]
        )
    split_rows[0]["query_family_ids"].append("repo-00.omitted")
    with pytest.raises(decision.DecisionError, match="differs from source split"):
        decision.replay_repository_disjoint_bundle(bundle_path)
    split_rows[0]["query_family_ids"].pop()
    original_bundle = bundle_path.read_bytes()
    missing = json.loads(original_bundle)
    missing["captures"].pop()
    bundle_path.write_text(json.dumps(missing))
    with pytest.raises(decision.DecisionError, match="inventory is incomplete"):
        decision.replay_repository_disjoint_bundle(bundle_path)
    bundle_path.write_bytes(original_bundle)

    def failed_verdict(*args):
        value = verdict(*args)
        value["states"]["PERF_QUALIFIED"] = "fail"
        return value

    monkeypatch.setattr(decision.run, "build_verdict", failed_verdict)
    with pytest.raises(decision.DecisionError, match="lacks qualified proof"):
        decision.replay_repository_disjoint_bundle(bundle_path)
    monkeypatch.setattr(decision.run, "build_verdict", verdict)

    report_path = tmp_path / "repo-00" / "report.json"
    original_report = report_path.read_bytes()
    ungraded = json.loads(original_report)
    ungraded["graded"] = False
    report_path.write_text(json.dumps(ungraded))
    with pytest.raises(ev.EvidenceError, match="not graded"):
        decision.replay_repository_disjoint_bundle(bundle_path)
    report_path.write_bytes(original_report)
    wrong_metric = json.loads(original_report)
    wrong_metric["rank_metric_version"] = "file-ranked-v1"
    report_path.write_text(json.dumps(wrong_metric))
    with pytest.raises(decision.DecisionError, match="report metric differs from policy"):
        decision.replay_repository_disjoint_bundle(bundle_path)
    report_path.write_bytes(original_report)

    (tmp_path / "repo-00" / "admission.json").write_text(
        json.dumps(
            {
                "schema_version": 3,
                "decision_policy_sha256": "0" * 64,
                "repository_commit": "1".zfill(40),
                "suite_sha256": policy["repository_scope"]["holdout"][0]["suite_sha256"],
                "repository_disjoint": {
                    "repository": "repo-00",
                    "release_digest": "sha256:" + "a" * 64,
                    "split_manifest_sha256": split_sha,
                },
            }
        )
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
