#!/usr/bin/env python3
"""Predeclared product-default decision after qualified retrieval evidence replay.

QUALITY_DELTA admits evidence, not an improvement. This separate gate requires
an admission-bound policy and refuses absent effect, stratum or resource proof.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

from tools.benchmark.evidence import parse_json, read_control
from tools.benchmark.retrieval import run
from tools.benchmark.retrieval.evaluator import qualified_query_family_ci
from tools.benchmark.retrieval.finite_json import is_finite_json_number


class DecisionError(ValueError):
    """A decision input is missing, malformed or not bound to the capture."""


def _object(value: object, keys: set[str], where: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise DecisionError(f"{where} requires exactly {sorted(keys)}")
    return value


def _name(value: object, where: str) -> str:
    if not isinstance(value, str) or not value:
        raise DecisionError(f"{where} must be a nonempty string")
    return value


def _number(value: object, where: str, *, positive: bool = False) -> float:
    if not is_finite_json_number(value) or (positive and value <= 0):
        raise DecisionError(
            f"{where} must be a finite {'positive' if positive else 'numeric'} value"
        )
    return float(value)


def validate_policy(value: object) -> dict:
    policy = _object(
        value,
        {
            "schema_version",
            "comparison",
            "min_useful_delta",
            "min_cluster_lower_95",
            "confidence_method",
            "critical_strata",
            "resource_limits",
        },
        "decision policy",
    )
    if type(policy["schema_version"]) is not int or policy["schema_version"] != 1:
        raise DecisionError("decision policy schema_version must be 1")
    comparison = _object(
        policy["comparison"],
        {"strategy", "baseline_route", "candidate_route", "primary_metric"},
        "comparison",
    )
    for key, value in comparison.items():
        _name(value, f"comparison.{key}")
    if comparison["primary_metric"] not in {"ndcg_at_10", "recall_at_10"}:
        raise DecisionError("comparison.primary_metric is unsupported")
    _number(policy["min_useful_delta"], "min_useful_delta", positive=True)
    lower = _number(policy["min_cluster_lower_95"], "min_cluster_lower_95")
    if (
        lower < 0
        or policy["confidence_method"] != "paired_query_family_cluster_bootstrap_percentile_v1"
    ):
        raise DecisionError("decision policy requires a nonnegative qualified cluster lower bound")
    strata = policy["critical_strata"]
    if not isinstance(strata, list) or not strata:
        raise DecisionError("critical_strata must predeclare at least one stratum")
    seen: set[tuple[str, str]] = set()
    for index, value in enumerate(strata):
        row = _object(value, {"axis", "name", "min_delta"}, f"critical_strata[{index}]")
        axis = _name(row["axis"], f"critical_strata[{index}].axis")
        name = _name(row["name"], f"critical_strata[{index}].name")
        if axis not in {"category", "language", "repository", "no_answer"}:
            raise DecisionError(f"critical_strata[{index}].axis is unsupported")
        if axis == "no_answer" and name != "all":
            raise DecisionError("no_answer critical stratum must be named all")
        if (axis, name) in seen:
            raise DecisionError("critical stratum is duplicated")
        seen.add((axis, name))
        _number(row["min_delta"], f"critical_strata[{index}].min_delta")
    limits = _object(
        policy["resource_limits"],
        {"max_query_p95_ms", "max_peak_rss_bytes", "max_index_bytes"},
        "resource_limits",
    )
    _number(limits["max_query_p95_ms"], "resource_limits.max_query_p95_ms", positive=True)
    for key in ("max_peak_rss_bytes", "max_index_bytes"):
        if type(limits[key]) is not int or limits[key] <= 0:
            raise DecisionError(f"resource_limits.{key} must be a positive integer")
    return policy


def evaluate_decision(
    policy: dict, verdict: dict, report: dict, cluster_ci: dict, measurements: dict
) -> dict:
    """Return a refusal unless every qualified, predeclared threshold passes."""
    validate_policy(policy)
    if not isinstance(verdict, dict) or not isinstance(verdict.get("states"), dict):
        raise DecisionError("qualified verdict is missing")
    if not isinstance(verdict.get("state_evidence"), dict):
        raise DecisionError("qualified verdict proof is missing")
    if not isinstance(verdict.get("comparisons"), list):
        raise DecisionError("qualified comparison inventory is missing")
    required = ("PAIR_VALID", "CONTRACT_GREEN", "SDK_PATH_GREEN", "QUALITY_DELTA", "PERF_QUALIFIED")
    for state in required:
        if verdict["states"].get(state) != "pass":
            raise DecisionError(f"{state} is not qualified")
        proof = verdict["state_evidence"].get(state)
        if not isinstance(proof, dict) or not run._is_hex(proof.get("proof_digest"), 64):
            raise DecisionError(f"{state} lacks a proof digest")
    if verdict.get("failure_class") != "none":
        raise DecisionError("qualified verdict has a failing claim")
    matches = [
        row
        for row in verdict["comparisons"]
        if isinstance(row, dict)
        and all(row.get(key) == value for key, value in policy["comparison"].items())
    ]
    if len(matches) != 1:
        raise DecisionError("predeclared comparison is absent or ambiguous")
    selected = matches[0]
    comparison = (
        report.get("rank_metrics", {}).get("comparison") if isinstance(report, dict) else None
    )
    if not isinstance(comparison, dict):
        raise DecisionError("bound rank comparison is missing")
    for policy_key, report_key in (
        ("baseline_route", "baseline"),
        ("candidate_route", "candidate"),
        ("primary_metric", "primary_metric"),
    ):
        if comparison.get(report_key) != policy["comparison"][policy_key]:
            raise DecisionError("bound report comparison differs from policy")
    delta = _number(comparison.get("primary_delta"), "primary_delta")
    if selected.get("primary_delta") != comparison["primary_delta"]:
        raise DecisionError("verdict and report primary deltas differ")
    if not isinstance(cluster_ci, dict) or cluster_ci.get("method") != policy["confidence_method"]:
        raise DecisionError("qualified cluster interval is missing")
    lower = _number(cluster_ci.get("lower_95"), "cluster lower_95")
    if not isinstance(measurements, dict) or set(measurements) != {
        "query_p95_ms",
        "peak_rss_bytes",
        "index_bytes",
    }:
        raise DecisionError("qualified resource observations are incomplete")
    p95 = _number(measurements["query_p95_ms"], "query_p95_ms")
    if p95 < 0:
        raise DecisionError("query_p95_ms observation must be nonnegative")
    for key in ("peak_rss_bytes", "index_bytes"):
        if type(measurements[key]) is not int or measurements[key] <= 0:
            raise DecisionError(f"{key} observation must be a positive integer")
    reasons = []
    if delta < policy["min_useful_delta"]:
        reasons.append("primary_effect_below_minimum")
    if lower < policy["min_cluster_lower_95"]:
        reasons.append("cluster_lower_bound_below_minimum")
    for row in policy["critical_strata"]:
        if row["axis"] == "no_answer":
            value = comparison.get("no_answer_abstention_delta", {}).get("mean_delta")
        else:
            value = (
                comparison.get("stratified_primary_delta", {})
                .get(row["axis"], {})
                .get(row["name"], {})
                .get("mean_delta")
            )
        observed = _number(value, f"critical stratum {row['axis']}:{row['name']}")
        if observed < row["min_delta"]:
            reasons.append(f"critical_stratum_regression:{row['axis']}:{row['name']}")
    limits = policy["resource_limits"]
    for actual, bound, reason in (
        (p95, limits["max_query_p95_ms"], "query_p95_budget_exceeded"),
        (measurements["peak_rss_bytes"], limits["max_peak_rss_bytes"], "peak_rss_budget_exceeded"),
        (measurements["index_bytes"], limits["max_index_bytes"], "index_budget_exceeded"),
    ):
        if actual > bound:
            reasons.append(reason)
    return {
        "decision_version": 1,
        "status": "refuse" if reasons else "admit",
        "reasons": reasons,
        "comparison": policy["comparison"],
        "primary_delta": delta,
        "cluster_lower_95": lower,
        "measurements": measurements,
        "report_digest": selected["report_digest"],
        "quality_proof_digest": verdict["state_evidence"]["QUALITY_DELTA"]["proof_digest"],
        "perf_proof_digest": verdict["state_evidence"]["PERF_QUALIFIED"]["proof_digest"],
    }


def build_decision(repo: Path, suite: Path, manifest_path: Path, policy_path: Path) -> dict:
    """Replay capture authority before inspecting selected effect and resources."""
    initial: dict[Path, str] = {}

    def bound_json(path: Path) -> object:
        raw = read_control(path)
        observed = hashlib.sha256(raw).hexdigest()
        expected = initial.setdefault(path, observed)
        if observed != expected:
            raise DecisionError("decision inputs changed during replay")
        return parse_json(raw.decode("utf-8"))

    root = manifest_path.resolve().parent
    manifest = run._validate_manifest_shape(bound_json(manifest_path))
    if manifest["scope"] != "qualified":
        raise DecisionError("default decision requires a qualified run")
    artifacts = manifest["artifacts"]
    admission_path = run._resolve_artifact(root, artifacts["admission_manifest"], "admission")
    admission = run.validate_admission_manifest(bound_json(admission_path))
    policy = validate_policy(bound_json(policy_path))
    policy_digest = initial[policy_path]
    if admission.get("decision_policy_sha256") != policy_digest:
        raise DecisionError("decision policy is not frozen in qualified admission")
    paths = {
        "matrix": run._resolve_artifact(root, artifacts["latency_matrix"], "latency_matrix"),
        "reports": [run._resolve_artifact(root, ref, "reports") for ref in artifacts["reports"]],
        "records": [run._resolve_artifact(root, ref, "records") for ref in artifacts["records"]],
        "resources": [
            run._resolve_artifact(root, ref, "resource_metrics")
            for ref in artifacts["resource_metrics"]
        ],
    }
    tracked = [
        manifest_path,
        admission_path,
        suite,
        policy_path,
        *paths["reports"],
        *paths["records"],
        *paths["resources"],
        paths["matrix"],
    ]
    for path in tracked:
        initial.setdefault(path, run.sha_file(path))
    verdict = run.build_verdict(repo, suite, manifest_path)
    selected_reports = [
        path
        for path in paths["reports"]
        if any(
            isinstance(row, dict)
            and all(row.get(key) == value for key, value in policy["comparison"].items())
            and row.get("report_digest") == run.sha_file(path)
            for row in verdict.get("comparisons", [])
        )
    ]
    if len(selected_reports) != 1:
        raise DecisionError("selected report does not match a verified comparison digest")
    report = bound_json(selected_reports[0])
    suite_payload = bound_json(suite)
    comparison = policy["comparison"]
    cluster_ci = qualified_query_family_ci(
        suite_payload, report, comparison["baseline_route"], comparison["candidate_route"]
    )
    matrix = bound_json(paths["matrix"])
    key = f"quanta:{comparison['strategy']}:{comparison['candidate_route']}"
    samples = matrix.get("samples") if isinstance(matrix, dict) else None
    floors = matrix.get("floors") if isinstance(matrix, dict) else None
    if not isinstance(samples, dict) or not isinstance(floors, dict):
        raise DecisionError("selected candidate latency observations are missing")
    candidate_samples = []
    for task_key, values in samples.items():
        if not isinstance(task_key, str) or not task_key.startswith(key + ":"):
            continue
        if not isinstance(values, list) or not values:
            raise DecisionError("candidate task has no latency observations")
        candidate_samples.extend(
            _number(value, f"candidate latency {task_key}") for value in values
        )
    if (
        not candidate_samples
        or type(floors.get(key)) is not int
        or floors[key] != len(candidate_samples)
        or any(value < 0 for value in candidate_samples)
    ):
        raise DecisionError("selected candidate latency floor or samples are invalid")
    by_subject = {}
    for path in paths["resources"]:
        value = bound_json(path)
        if (
            not isinstance(value, dict)
            or not run._is_hex(value.get("subject_sha256"), 64)
            or value["subject_sha256"] in by_subject
        ):
            raise DecisionError("resource subject is missing or duplicated")
        by_subject[value["subject_sha256"]] = value
    selected_resources = []
    for path in paths["records"]:
        record = bound_json(path)
        if run._record_identity(record, str(path)) != ("quanta", comparison["strategy"]):
            continue
        metric = by_subject.get(run.sha_file(path))
        if metric is None:
            raise DecisionError("selected record lacks bound resource metrics")
        selected_resources.append(metric)
    if len(selected_resources) != manifest["repetitions"]:
        raise DecisionError("selected resource roots do not match qualified repetitions")
    measurements = {
        "query_p95_ms": run.latency_summary(candidate_samples)["p95_ms"],
        "peak_rss_bytes": max(item.get("peak_rss_bytes", 0) for item in selected_resources),
        "index_bytes": max(
            item.get("storage", {}).get("index_bytes", 0) for item in selected_resources
        ),
    }
    decision = evaluate_decision(policy, verdict, report, cluster_ci, measurements)
    if any(run.sha_file(path) != before for path, before in initial.items()):
        raise DecisionError("decision inputs changed during replay")
    return {
        **decision,
        "policy_sha256": policy_digest,
        "admission_sha256": run.sha_file(admission_path),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for key in ("repo", "suite", "run-manifest", "policy", "out"):
        parser.add_argument(f"--{key}", required=True)
    args = parser.parse_args(argv)
    try:
        decision = build_decision(
            Path(args.repo), Path(args.suite), Path(args.run_manifest), Path(args.policy)
        )
    except (DecisionError, run.RunError, ValueError, OSError, KeyError, TypeError) as exc:
        print(f"ERROR: default decision refused: {exc}", file=sys.stderr)
        return 2
    Path(args.out).write_text(
        json.dumps(decision, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps({"status": decision["status"], "reasons": decision["reasons"]}))
    return 0 if decision["status"] == "admit" else 1


if __name__ == "__main__":
    raise SystemExit(main())
