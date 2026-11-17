#!/usr/bin/env python3
"""Predeclared product-default decision after qualified retrieval evidence replay.

QUALITY_DELTA admits evidence, not an improvement. This separate gate requires
an admission-bound policy and refuses absent effect, stratum or resource proof.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
import sys
from pathlib import Path

from tools.benchmark import evidence as benchmark_evidence
from tools.benchmark.evidence import parse_json, read_control
from tools.benchmark.retrieval import (
    evaluator,
    go_trace_review_receipt,
    query_plan,
    result_window,
    run,
)
from tools.benchmark.retrieval.evaluator import qualified_query_family_ci
from tools.benchmark.retrieval.finite_json import is_finite_json_number


class DecisionError(ValueError):
    """A decision input is missing, malformed or not bound to the capture."""


_CONTEXT_METRICS = frozenset({"ndcg_at_10", "recall_at_10"})
_FILE_METRICS = frozenset({"file_ndcg_at_10", "file_mrr_at_10", "file_recall_at_10"})


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


def _validate_policy(value: object, allowed_metrics: frozenset[str]) -> dict:
    policy = _object(
        value,
        {
            "schema_version",
            "repository_scope",
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
    scope = _object(
        policy["repository_scope"],
        {"kind", "repository_commit"},
        "repository_scope",
    )
    if scope["kind"] != "single_repository":
        raise DecisionError(
            "multi-repository decision requires repository-bound capture, "
            "cell inventory, and repository-cluster uncertainty"
        )
    if not run._is_hex(scope["repository_commit"], 40):
        raise DecisionError("repository_scope.repository_commit must be a full Git SHA")
    comparison = _object(
        policy["comparison"],
        {"strategy", "baseline_route", "candidate_route", "primary_metric"},
        "comparison",
    )
    for key, value in comparison.items():
        _name(value, f"comparison.{key}")
    if comparison["primary_metric"] not in allowed_metrics:
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
    if ("no_answer", "all") not in seen:
        raise DecisionError("critical_strata must include no_answer:all")
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


def validate_policy(value: object) -> dict:
    return _validate_policy(value, _CONTEXT_METRICS)


def _absolute_path(value: object, where: str) -> Path:
    if (
        not isinstance(value, str)
        or not value
        or "\\" in value
        or "\x00" in value
        or not Path(value).is_absolute()
        or ".." in Path(value).parts
        or Path(value).as_posix() != value
    ):
        raise DecisionError(f"{where} must be a canonical absolute path")
    return Path(value)


def _validate_split_manifest(raw: bytes, releases: dict[str, Path]) -> dict:
    """Load the legacy benchmark package only for multi-repository replay."""
    benchmark_dir = str(Path(__file__).resolve().parents[1])
    if benchmark_dir not in sys.path:
        sys.path.insert(0, benchmark_dir)
    from tools.benchmark import corpus_binding

    return corpus_binding.validate_split_manifest(raw, releases)


def validate_repository_disjoint_policy(value: object) -> dict:
    """Validate the predeclared C5 policy without granting a product decision."""
    if not isinstance(value, dict) or type(value.get("schema_version")) is not int:
        raise DecisionError("repository-disjoint policy schema_version is invalid")
    version = value["schema_version"]
    if version not in (2, 3):
        raise DecisionError("repository-disjoint policy schema_version must be 2 or 3")
    file_policy = version == 3
    policy = _object(
        value,
        {
            "schema_version",
            "repository_scope",
            "comparison",
            "min_useful_delta",
            "min_cluster_lower_95",
            "confidence_method",
            "critical_strata",
            "track_min_delta",
            "resource_limits",
        }
        | ({"metric_scope", "request_mode"} if file_policy else set()),
        "repository-disjoint decision policy",
    )
    if file_policy and (
        not isinstance(policy["comparison"], dict)
        or policy["metric_scope"] != "scored_distinct_file"
        or policy["request_mode"] != run.qp.DEFAULT_FILE_SEARCH
        or policy["comparison"].get("primary_metric") not in _FILE_METRICS
    ):
        raise DecisionError("repository-disjoint file metric or request mode is unsupported")
    scope = _object(
        policy["repository_scope"],
        {"kind", "split_manifest_sha256", "releases", "holdout"},
        "repository-disjoint scope",
    )
    if scope["kind"] != "repository_disjoint_holdout" or not run._is_hex(
        scope["split_manifest_sha256"], 64
    ):
        raise DecisionError("repository-disjoint split identity is invalid")
    releases = scope["releases"]
    if not isinstance(releases, dict) or not releases:
        raise DecisionError("repository-disjoint releases are missing")
    for digest, path in releases.items():
        if (
            not isinstance(digest, str)
            or not digest.startswith("sha256:")
            or not run._is_hex(digest.removeprefix("sha256:"), 64)
        ):
            raise DecisionError("repository-disjoint release digest is invalid")
        _absolute_path(path, "repository-disjoint release")
    holdout = scope["holdout"]
    if not isinstance(holdout, list) or len(holdout) < 12:
        raise DecisionError("repository-disjoint holdout needs at least twelve repositories")
    names, commits, strata, families = [], set(), {}, set()
    for index, raw in enumerate(holdout):
        row = _object(
            raw,
            {
                "repository",
                "repository_commit",
                "release_digest",
                "stratum",
                "suite_sha256",
                "query_family_ids",
                "categories",
            },
            f"repository-disjoint holdout[{index}]",
        )
        name = _name(row["repository"], "holdout repository")
        commit = row["repository_commit"]
        if not run._is_hex(commit, 40) or commit in commits:
            raise DecisionError("repository-disjoint holdout commit is invalid or duplicate")
        if row["release_digest"] not in releases or not run._is_hex(row["suite_sha256"], 64):
            raise DecisionError("repository-disjoint holdout release or suite digest differs")
        stratum = _name(row["stratum"], "holdout stratum")
        for key in ("query_family_ids", "categories"):
            values = row[key]
            if (
                not isinstance(values, list)
                or not values
                or any(not isinstance(item, str) or not item for item in values)
                or values != sorted(set(values))
            ):
                raise DecisionError(f"repository-disjoint holdout {key} is invalid")
        if families.intersection(row["query_family_ids"]):
            raise DecisionError("repository-disjoint query family crosses repositories")
        families.update(row["query_family_ids"])
        names.append(name)
        commits.add(commit)
        strata[stratum] = strata.get(stratum, 0) + 1
    if names != sorted(set(names)) or any(count < 2 for count in strata.values()):
        raise DecisionError("repository-disjoint holdout names or strata are invalid")
    tracks = _object(
        policy["track_min_delta"], {"objective", "reviewed"}, "repository-disjoint tracks"
    )
    for name, minimum in tracks.items():
        _number(minimum, f"repository-disjoint track {name} minimum")
    single = {
        **{
            key: value
            for key, value in policy.items()
            if key not in {"track_min_delta", "metric_scope", "request_mode"}
        },
        "schema_version": 1,
        "repository_scope": {
            "kind": "single_repository",
            "repository_commit": holdout[0]["repository_commit"],
        },
        "confidence_method": "paired_query_family_cluster_bootstrap_percentile_v1",
    }
    _validate_policy(single, _FILE_METRICS if file_policy else _CONTEXT_METRICS)
    if (
        policy["confidence_method"]
        != "paired_stratified_repository_cluster_bootstrap_percentile_v1"
    ):
        raise DecisionError("repository-disjoint uncertainty method is unsupported")
    required_repo_strata = {(row["axis"], row["name"]) for row in policy["critical_strata"]}
    if any(("repository", name) not in required_repo_strata for name in names):
        raise DecisionError("repository-disjoint critical strata omit a holdout repository")
    return policy


def _repository_disjoint_track(task: dict) -> str:
    """Classify positive gold using the validated suite's label authority."""
    if task.get("source_oracle") is not None:
        if (
            task.get("judgment_policy") != evaluator.SOURCE_ORACLE_JUDGMENT_POLICY
            or "label_review" in task
        ):
            raise DecisionError("repository-disjoint objective label authority is invalid")
        return "objective"
    review = task.get("label_review")
    if (
        isinstance(review, dict)
        and review.get("assessment") in evaluator.LABEL_REVIEW_ASSESSMENTS[1:]
        and task.get("judgment_policy") == evaluator.COMPLETE_JUDGMENT_POLICY
        and ("file_judgments" in task or "declaration_judgments" in task)
    ):
        return "reviewed"
    raise DecisionError("repository-disjoint positive task lacks objective or reviewed labels")


def _repository_disjoint_metric_gate(
    policy: dict,
    suites: dict[str, dict],
    reports: dict[str, dict],
    ci: dict,
    receipts: list[dict],
) -> dict:
    """Apply preregistered thresholds after the input replay has succeeded."""
    scope = policy["repository_scope"]
    names = [row["repository"] for row in scope["holdout"]]
    if (
        ci.get("status") != "available"
        or ci.get("method") != policy["confidence_method"]
        or ci.get("repository_count") != len(names)
        or ci.get("aggregation") != "equal_family_within_repository_equal_repository"
        or [row.get("repository") for row in receipts] != names
    ):
        raise DecisionError("repository-disjoint interval or resource coverage differs")
    primary = _number(ci.get("mean"), "repository-disjoint primary effect")
    lower = _number(ci.get("lower_95"), "repository-disjoint lower bound")
    family_rows = []
    no_answer = {}
    for name in names:
        suite, report = suites[name], reports[name]
        task_by_id = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
        repository_tracks = set()
        for task_id, family_id, category, delta in evaluator.paired_query_family_rows(
            suite,
            report,
            policy["comparison"]["baseline_route"],
            policy["comparison"]["candidate_route"],
            require_graded=True,
        ):
            task = task_by_id[task_id]
            track = _repository_disjoint_track(task)
            repository_tracks.add(track)
            family_rows.append(
                (name, family_id, category, evaluator._task_language(task), track, delta)
            )
        if repository_tracks != {"objective", "reviewed"}:
            raise DecisionError("repository-disjoint objective or reviewed track is missing")
        comparison = report["rank_metrics"]["comparison"]
        negative_tasks = [
            task
            for task in suite["tasks"]
            if task["split"] == "eval"
            and not evaluator.rank_task_answerable(task, policy["comparison"]["primary_metric"])
        ]
        negative_count = len(negative_tasks)
        negative = comparison.get("no_answer_abstention_delta")
        query_rows = {(item["task_id"], item["route"]): item for item in report["per_query"]}
        observed_negative = math.fsum(
            float(
                query_rows[(task["task_id"], policy["comparison"]["candidate_route"])]["status"]
                == "abstained"
            )
            - float(
                query_rows[(task["task_id"], policy["comparison"]["baseline_route"])]["status"]
                == "abstained"
            )
            for task in negative_tasks
        )
        if (
            negative_count == 0
            or not isinstance(negative, dict)
            or type(negative.get("sample_count")) is not int
            or negative["sample_count"] != negative_count
        ):
            raise DecisionError("repository-disjoint no-answer coverage is incomplete")
        no_answer[name] = _number(negative.get("mean_delta"), "no-answer delta")
        if not math.isclose(no_answer[name], observed_negative / negative_count, abs_tol=1e-12):
            raise DecisionError("repository-disjoint no-answer mean differs from report rows")
    if ci.get("sample_count") != len(family_rows):
        raise DecisionError("repository-disjoint interval task coverage differs")
    critical = {(row["axis"], row["name"]) for row in policy["critical_strata"]}
    observed_strata = {
        (axis, value)
        for _repository, _family, category, language, _track, _delta in family_rows
        for axis, value in (("category", category), ("language", language))
    }
    if not observed_strata <= critical:
        raise DecisionError("repository-disjoint policy omits an observed critical stratum")

    def mean_for(axis: str, value: str) -> tuple[float, int]:
        groups = {}
        for repository, family, category, language, track, delta in family_rows:
            if axis == "repository" and repository != value:
                continue
            if axis == "category" and category != value:
                continue
            if axis == "language" and language != value:
                continue
            if axis == "track" and track != value:
                continue
            groups.setdefault(repository, {}).setdefault(family, []).append(delta)
        if not groups:
            raise DecisionError(f"repository-disjoint critical stratum {axis}:{value} is empty")
        means = [
            math.fsum(math.fsum(group) / len(group) for _family, group in sorted(families.items()))
            / len(families)
            for _repository, families in sorted(groups.items())
        ]
        return math.fsum(means) / len(means), len(means)

    overall, observed_repositories = mean_for("all", "all")
    if observed_repositories != len(names) or not math.isclose(overall, primary, abs_tol=1e-12):
        raise DecisionError("repository-disjoint interval mean differs from report rows")
    reasons = []
    if primary < policy["min_useful_delta"]:
        reasons.append("primary_effect_below_minimum")
    if lower < policy["min_cluster_lower_95"]:
        reasons.append("repository_lower_bound_below_minimum")
    for row in policy["critical_strata"]:
        axis, name = row["axis"], row["name"]
        if axis == "no_answer":
            observed, coverage = math.fsum(no_answer.values()) / len(no_answer), len(no_answer)
        else:
            observed, coverage = mean_for(axis, name)
        if coverage < (1 if axis == "repository" else 2):
            raise DecisionError(f"repository-disjoint critical stratum {axis}:{name} is sparse")
        if observed < row["min_delta"]:
            reasons.append(f"critical_stratum_regression:{axis}:{name}")
    track_summary = {}
    for name, minimum in policy["track_min_delta"].items():
        observed, coverage = mean_for("track", name)
        if coverage != len(names):
            raise DecisionError(f"repository-disjoint {name} track coverage is incomplete")
        track_summary[name] = {"repository_count": coverage, "mean_delta": observed}
        if observed < minimum:
            reasons.append(f"track_regression:{name}")
    measurements = {}
    for key in ("query_p95_ms", "peak_rss_bytes", "index_bytes"):
        values = [receipt["measurements"].get(key) for receipt in receipts]
        if any(
            not is_finite_json_number(value)
            or value < 0
            or (key != "query_p95_ms" and (type(value) is not int or value == 0))
            for value in values
        ):
            raise DecisionError(f"repository-disjoint {key} observation is invalid")
        measurements[key] = max(values)
    limits = policy["resource_limits"]
    for key, reason in (
        ("query_p95_ms", "query_p95_budget_exceeded"),
        ("peak_rss_bytes", "peak_rss_budget_exceeded"),
        ("index_bytes", "index_budget_exceeded"),
    ):
        if measurements[key] > limits["max_" + key]:
            reasons.append(reason)
    return {
        "status": "refuse" if reasons else "eligible_for_human_review",
        "reasons": reasons,
        "primary_delta": primary,
        "repository_lower_95": lower,
        "measurements": measurements,
        "tracks": track_summary,
    }


def replay_repository_disjoint_bundle(bundle_path: Path) -> dict:
    """Replay every predeclared holdout capture before computing uncertainty.

    This is an input gate. It does not make a product-default decision or
    establish that self-reported reviewer identities are human.
    """
    initial: dict[Path, str] = {}

    def bound_bytes(path: Path) -> bytes:
        raw = read_control(path)
        observed = hashlib.sha256(raw).hexdigest()
        expected = initial.setdefault(path, observed)
        if observed != expected:
            raise DecisionError("repository-disjoint inputs changed during replay")
        return raw

    def bound_json(path: Path) -> dict:
        value = parse_json(bound_bytes(path).decode("utf-8"))
        if not isinstance(value, dict):
            raise DecisionError("repository-disjoint input must be a JSON object")
        return value

    bundle = _object(
        bound_json(bundle_path),
        {"schema_version", "kind", "policy", "split_manifest", "captures"},
        "repository-disjoint bundle",
    )
    if bundle["schema_version"] != 1 or bundle["kind"] != "repository_disjoint_c5_inputs":
        raise DecisionError("repository-disjoint bundle version or kind differs")
    policy_path = _absolute_path(bundle["policy"], "repository-disjoint policy")
    split_path = _absolute_path(bundle["split_manifest"], "repository-disjoint split")
    policy = validate_repository_disjoint_policy(bound_json(policy_path))
    policy_sha = initial[policy_path]
    scope = policy["repository_scope"]
    split_raw = bound_bytes(split_path)
    if hashlib.sha256(split_raw).hexdigest() != scope["split_manifest_sha256"]:
        raise DecisionError("repository-disjoint split differs from frozen policy")
    releases = {digest: Path(path) for digest, path in scope["releases"].items()}
    split = _validate_split_manifest(split_raw, releases)
    split_holdout = {
        row["repository"]: row for row in split["repositories"] if row["split"] == "holdout"
    }
    expected = {row["repository"]: row for row in scope["holdout"]}
    if set(split_holdout) != set(expected):
        raise DecisionError("repository-disjoint policy differs from split holdout roster")
    for name, row in expected.items():
        source = split_holdout[name]
        if (
            row["repository_commit"] != source["repository_commit"]
            or row["release_digest"] != source["release_digest"]
            or row["query_family_ids"] != source["query_family_ids"]
        ):
            raise DecisionError("repository-disjoint policy differs from source split")
    captures = bundle["captures"]
    if not isinstance(captures, list) or len(captures) != len(expected):
        raise DecisionError("repository-disjoint capture inventory is incomplete")
    names = []
    suites, reports, receipts = {}, {}, []
    comparison = policy["comparison"]
    required_states = (
        "PAIR_VALID",
        "CONTRACT_GREEN",
        "SDK_PATH_GREEN",
        "QUALITY_DELTA",
        "PERF_QUALIFIED",
    )
    for index, raw in enumerate(captures):
        entry = _object(
            raw,
            {"repository", "checkout", "suite", "run_manifest"},
            f"repository-disjoint capture[{index}]",
        )
        name = _name(entry["repository"], "capture repository")
        names.append(name)
        if name not in expected:
            raise DecisionError("repository-disjoint capture has an unknown repository")
        checkout = _absolute_path(entry["checkout"], "capture checkout")
        suite_path = _absolute_path(entry["suite"], "capture suite")
        manifest_path = _absolute_path(entry["run_manifest"], "capture manifest")
        suite = bound_json(suite_path)
        row = expected[name]
        source = split_holdout[name]
        tasks = suite.get("tasks")
        if (
            initial[suite_path] != row["suite_sha256"]
            or suite.get("repository_commit") != row["repository_commit"]
            or not run._is_hex(suite.get("file_universe_digest"), 64)
            or "sha256:" + suite["file_universe_digest"] != source["code_only_universe_digest"]
            or not isinstance(tasks, list)
            or not tasks
        ):
            raise DecisionError("repository-disjoint suite differs from frozen policy or source")
        if any(
            not isinstance(task, dict)
            or task.get("split") not in {"train", "eval"}
            or not isinstance(task.get("query_family_id"), str)
            or not isinstance(task.get("category"), str)
            for task in tasks
        ):
            raise DecisionError("repository-disjoint task inventory is malformed")
        eval_tasks = [task for task in tasks if task["split"] == "eval"]
        if not eval_tasks:
            raise DecisionError("repository-disjoint suite lacks eval tasks")
        if (
            sorted({task.get("query_family_id") for task in eval_tasks}) != row["query_family_ids"]
            or sorted({task.get("category") for task in eval_tasks}) != row["categories"]
        ):
            raise DecisionError("repository-disjoint suite omits a predeclared family or category")
        if policy["schema_version"] == 3:
            contract = evaluator.declared_evaluation_contract(tasks)
            if contract != {
                "request_mode": policy["request_mode"],
                "gold_unit": "distinct_file",
                "result_unit": "distinct_file",
            }:
                raise DecisionError("repository-disjoint file request mode differs")
        manifest = run._validate_manifest_shape(bound_json(manifest_path))
        if manifest["scope"] != "qualified":
            raise DecisionError("repository-disjoint capture is not qualified")
        root = manifest_path.resolve().parent
        admission_path = run._resolve_artifact(
            root, manifest["artifacts"]["admission_manifest"], "admission"
        )
        admission = run.validate_admission_manifest(bound_json(admission_path))
        disjoint = admission.get("repository_disjoint")
        if (
            admission.get("schema_version") != 3
            or not isinstance(disjoint, dict)
            or disjoint.get("repository") != name
            or disjoint.get("release_digest") != row["release_digest"]
            or disjoint.get("split_manifest_sha256") != scope["split_manifest_sha256"]
            or admission.get("decision_policy_sha256") != policy_sha
            or admission["repository_commit"] != row["repository_commit"]
            or admission["suite_sha256"] != initial[suite_path]
        ):
            raise DecisionError("repository-disjoint policy is not frozen in capture admission")
        verdict = run.build_verdict(checkout, suite_path, manifest_path)
        if (
            verdict.get("failure_class") != "none"
            or not isinstance(verdict.get("states"), dict)
            or not isinstance(verdict.get("state_evidence"), dict)
            or any(verdict["states"].get(state) != "pass" for state in required_states)
            or any(
                not isinstance(verdict["state_evidence"].get(state), dict)
                or not run._is_hex(verdict["state_evidence"][state].get("proof_digest"), 64)
                for state in required_states
            )
            or not isinstance(verdict.get("comparisons"), list)
        ):
            raise DecisionError("repository-disjoint capture lacks qualified proof")
        selected = [
            item
            for item in verdict.get("comparisons", [])
            if isinstance(item, dict)
            and all(item.get(key) == value for key, value in comparison.items())
            and item.get("graded") is True
        ]
        if len(selected) != 1 or not run._is_hex(selected[0].get("report_digest"), 64):
            raise DecisionError("repository-disjoint selected comparison is absent or ambiguous")
        report_paths = [
            run._resolve_artifact(root, ref, "reports") for ref in manifest["artifacts"]["reports"]
        ]
        matching = [
            path
            for path in report_paths
            if hashlib.sha256(bound_bytes(path)).hexdigest() == selected[0]["report_digest"]
        ]
        if len(matching) != 1:
            raise DecisionError("repository-disjoint report differs from verified comparison")
        report = bound_json(matching[0])
        rank = report.get("rank_metrics")
        observed = rank.get("comparison") if isinstance(rank, dict) else None
        file_metric = policy["schema_version"] == 3
        if (
            report.get("rank_metric_version")
            != (
                "file-judgments-complete-v1"
                if file_metric
                else "rb-rank-context-density-first-coverage"
            )
            or file_metric
            and (
                report.get("report_scope") != "paired_complete_scored_file_evidence_v1"
                or report.get("status") != "evidence_unqualified"
            )
            or not file_metric
            and report.get("report_scope") == "paired_independent_file_judgment_diagnostic_v1"
            or not isinstance(observed, dict)
            or observed.get("primary_metric") != comparison["primary_metric"]
            or observed.get("primary_delta") != selected[0].get("primary_delta")
            or observed.get("sample_count") != selected[0].get("sample_count")
        ):
            raise DecisionError("repository-disjoint report metric differs from policy")
        paths = {
            "matrix": run._resolve_artifact(
                root, manifest["artifacts"]["latency_matrix"], "latency_matrix"
            ),
            "records": [
                run._resolve_artifact(root, ref, "records")
                for ref in manifest["artifacts"]["records"]
            ],
            "resources": [
                run._resolve_artifact(root, ref, "resource_metrics")
                for ref in manifest["artifacts"]["resource_metrics"]
            ],
        }
        measurements = _candidate_measurements(manifest, paths, comparison, bound_json)
        suites[name], reports[name] = suite, report
        receipts.append(
            {
                "repository": name,
                "admission_sha256": initial[admission_path],
                "report_sha256": selected[0]["report_digest"],
                "measurements": measurements,
            }
        )
    if names != sorted(expected):
        raise DecisionError("repository-disjoint capture inventory is duplicate or unsorted")
    corpus_digest = evaluator.digest(
        evaluator.canonical(
            {"split_manifest_sha256": scope["split_manifest_sha256"], "releases": sorted(releases)}
        )
    )
    ci = evaluator.repository_cluster_ci_from_reports(
        suites,
        reports,
        corpus_digest,
        {name: row["repository_commit"] for name, row in expected.items()},
        {name: row["stratum"] for name, row in expected.items()},
        comparison["baseline_route"],
        comparison["candidate_route"],
    )
    if ci.get("status") != "available":
        raise DecisionError("repository-disjoint uncertainty is not estimable")
    metric_gate = _repository_disjoint_metric_gate(policy, suites, reports, ci, receipts)
    if any(
        hashlib.sha256(read_control(path)).hexdigest() != before for path, before in initial.items()
    ):
        raise DecisionError("repository-disjoint inputs changed during replay")
    return {
        "schema_version": 1,
        "status": "replayed_no_default_decision",
        "product_default_decision": False,
        "policy_sha256": policy_sha,
        "split_manifest_sha256": scope["split_manifest_sha256"],
        "repository_count": len(expected),
        "paired_sample_count": ci["sample_count"],
        "repository_cluster_ci": ci,
        "metric_scope": policy.get("metric_scope", "context_span_density"),
        "metric_gate": metric_gate,
        "captures": receipts,
    }


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
    if selected.get("graded") is not True:
        raise DecisionError("predeclared comparison is not graded")
    comparison = (
        report.get("rank_metrics", {}).get("comparison") if isinstance(report, dict) else None
    )
    if not isinstance(comparison, dict):
        raise DecisionError("bound rank comparison is missing")
    if report.get("repository_commit") != policy["repository_scope"]["repository_commit"]:
        raise DecisionError("bound report repository differs from policy")
    if report.get("graded") is not True:
        raise DecisionError("bound report is not graded")
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
    count = comparison.get("sample_count")
    if type(count) is not int or count <= 0 or selected.get("sample_count") != count:
        raise DecisionError("predeclared comparison lacks paired graded coverage")
    if not isinstance(cluster_ci, dict) or cluster_ci.get("method") != policy["confidence_method"]:
        raise DecisionError("qualified cluster interval is missing")
    if (
        type(cluster_ci.get("sample_count")) is not int
        or cluster_ci["sample_count"] != count
        or type(cluster_ci.get("cluster_count")) is not int
        or cluster_ci["cluster_count"] <= 0
    ):
        raise DecisionError("qualified cluster interval lacks paired coverage")
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
            stratum = comparison.get("no_answer_abstention_delta", {})
        else:
            stratum = (
                comparison.get("stratified_primary_delta", {})
                .get(row["axis"], {})
                .get(row["name"], {})
            )
        if (
            not isinstance(stratum, dict)
            or type(stratum.get("sample_count")) is not int
            or stratum["sample_count"] <= 0
        ):
            raise DecisionError(f"critical stratum {row['axis']}:{row['name']} lacks coverage")
        value = stratum.get("mean_delta")
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
        "repository_scope": policy["repository_scope"],
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


def _candidate_measurements(manifest: dict, paths: dict, comparison: dict, bound_json) -> dict:
    """Read replay-bound latency and resource observations for one candidate."""
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
    return {
        "query_p95_ms": run.latency_summary(candidate_samples)["p95_ms"],
        "peak_rss_bytes": max(item.get("peak_rss_bytes", 0) for item in selected_resources),
        "index_bytes": max(
            item.get("storage", {}).get("index_bytes", 0) for item in selected_resources
        ),
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
    if (
        not isinstance(suite_payload, dict)
        or suite_payload.get("repository_commit") != policy["repository_scope"]["repository_commit"]
    ):
        raise DecisionError("bound suite repository differs from policy")
    comparison = policy["comparison"]
    cluster_ci = qualified_query_family_ci(
        suite_payload, report, comparison["baseline_route"], comparison["candidate_route"]
    )
    measurements = _candidate_measurements(manifest, paths, comparison, bound_json)
    decision = evaluate_decision(policy, verdict, report, cluster_ci, measurements)
    if any(run.sha_file(path) != before for path, before in initial.items()):
        raise DecisionError("decision inputs changed during replay")
    return {
        **decision,
        "policy_sha256": policy_digest,
        "admission_sha256": run.sha_file(admission_path),
    }


def _replay_go_trace_file_page(
    trace: dict, repo: str, universe: set[str], lane_returned: dict[str, int],
    frame_path: str | None, task_id: str,
) -> list[dict]:
    pools = _object(trace.get("rank_pools"), set(lane_returned), "Go trace rank pools")
    first = {}
    for lane, returned in lane_returned.items():
        pool = pools[lane]
        if not isinstance(pool, list) or len(pool) != returned:
            raise DecisionError(f"Go trace {lane} rank pool count differs from its window: {task_id}")
        witnesses = {}
        for chunk_rank, raw in enumerate(pool, 1):
            row = _object(raw, {"file", "candidate_id", "score_bits"}, f"Go trace {lane} pool row")
            file = _object(row["file"], {"source_repo_id", "repo_relative_path"},
                           f"Go trace {lane} pool file")
            path, bits, candidate_id = file["repo_relative_path"], row["score_bits"], row["candidate_id"]
            if (
                file["source_repo_id"] != repo
                or not isinstance(path, str) or path not in universe
                or not isinstance(candidate_id, str) or not candidate_id
                or type(bits) is not int or not 0 <= bits <= 0xFFFF_FFFF
                or bits & 0x7F80_0000 == 0x7F80_0000
            ):
                raise DecisionError(f"Go trace {lane} pool row is invalid: {task_id}")
            if path not in witnesses:
                witnesses[path] = (len(witnesses) + 1, chunk_rank, candidate_id, bits)
        first[lane] = witnesses

    ranked = []
    for path in first["lexical"].keys() | first["dense"].keys():
        lexical, dense = first["lexical"].get(path), first["dense"].get(path)
        score = sum(1.0 / (60.0 + witness[0]) for witness in (lexical, dense) if witness)
        ranked.append((path != frame_path, -score, lexical is None, path))
    ranked.sort()
    expected = ranked[:20]
    candidates = trace.get("candidates")
    if not isinstance(candidates, list) or len(candidates) != len(expected):
        raise DecisionError(f"Go trace file page differs from lane pool replay: {task_id}")

    for candidate, (_, negative_score, _, path) in zip(candidates, expected):
        row = _object(candidate, {"file", "source_repo_id", "path", "candidate_id",
                                  "file_rrf_score", "production_source", "source_frame",
                                  "lexical", "dense"}, "Go trace file candidate")
        if row["path"] != path:
            raise DecisionError(f"Go trace file page differs from lane pool replay: {task_id}")
        if (row["file"] != {"repo_relative_path": path, "source_repo_id": repo}
                or row["source_repo_id"] != repo
                or row["production_source"] is not (path.endswith(".go") and not path.endswith("_test.go"))):
            raise DecisionError(f"Go trace file rank provenance is invalid: {task_id}")
        if row["source_frame"] is True and frame_path is None:
            raise DecisionError(f"Go trace frame was promoted without exact owner: {task_id}")
        if row["source_frame"] is not (path == frame_path):
            raise DecisionError(f"Go trace promoted path disagrees with failure excerpt: {task_id}")
        for lane, by_path in first.items():
            expected_witness = by_path.get(path)
            actual = row[lane]
            if expected_witness is None and actual is None:
                continue
            if expected_witness is None or not isinstance(actual, dict):
                raise DecisionError(f"Go trace {lane} witness rank is invalid: {task_id}")
            file_rank, chunk_rank, candidate_id, score_bits = expected_witness
            actual = _object(actual, {"file_rank", "chunk_rank", "candidate_id", "path",
                                      "source_repo_id", "raw_score"}, f"Go trace {lane} witness")
            if (type(actual["file_rank"]) is not int or actual["file_rank"] != file_rank
                    or type(actual["chunk_rank"]) is not int or actual["chunk_rank"] != chunk_rank):
                raise DecisionError(f"Go trace {lane} witness rank is invalid: {task_id}")
            raw_score = actual["raw_score"]
            if (not is_finite_json_number(raw_score) or actual["candidate_id"] != candidate_id
                    or actual["source_repo_id"] != repo or actual["path"] != path):
                raise DecisionError(f"Go trace {lane} witness drift: {task_id}")
            try:
                packed_score = struct.pack("<f", raw_score)
            except OverflowError as exc:
                raise DecisionError(f"Go trace {lane} witness drift: {task_id}") from exc
            if packed_score != score_bits.to_bytes(4, "little"):
                raise DecisionError(f"Go trace {lane} witness drift: {task_id}")
        representative = first["lexical"].get(path) or first["dense"][path]
        if row["candidate_id"] != representative[2]:
            raise DecisionError(f"Go trace representative identity drift: {task_id}")
        if not is_finite_json_number(row["file_rrf_score"]) or row["file_rrf_score"] != -negative_score:
            raise DecisionError(f"Go trace RRF score disagrees with lane ranks: {task_id}")
    return [{"path": path} for _, _, _, path in expected]


def evaluate_go_trace_file_holdout(
    plan_path: Path, judgments_path: Path, out_path: Path | None = None
) -> dict:
    """Score complete, source-bound Go SDK files without granting default adoption.

    Incomplete labels or attempts refuse before any aggregate is returned. The
    older known-file diagnostic remains a separate descriptive artifact.
    """
    from tools.benchmark.retrieval.arb_official_score import read_strict

    plan, plan_raw = read_strict(plan_path)
    judgments, judgments_raw = read_strict(judgments_path)
    if not isinstance(plan, dict) or not isinstance(judgments, dict):
        raise DecisionError("Go trace plan and judgments must be objects")
    plan_status = plan.get("status")
    if not isinstance(plan_status, str) or plan_status not in {
        "FROZEN_GO_TRACE_HOLDOUT",
        "FROZEN_GO_TRACE_HOLDOUT_REVIEWED_V1",
    }:
        raise DecisionError("Go trace plan has no frozen holdout admission")
    reviewed = plan_status == "FROZEN_GO_TRACE_HOLDOUT_REVIEWED_V1"
    denominator = judgments.get("recall_denominator")
    if not isinstance(denominator, dict) or denominator.get("genuinely_exhaustive") is not True:
        raise DecisionError("Go trace recall denominator is not exhaustive")
    source_head = _name(plan.get("source_head"), "Go trace policy source head")
    if not run._is_hex(source_head, 40):
        raise DecisionError("Go trace policy source head is invalid")
    build_binding = plan.get("build_binding_sha256")
    if not run._is_hex(build_binding, 64):
        raise DecisionError("Go trace plan lacks a frozen build binding")
    if not run._is_hex(plan.get("input_sha256"), 64):
        raise DecisionError("Go trace plan lacks frozen input identity")
    plans = plan.get("plans")
    cases = judgments.get("cases")
    if not isinstance(plans, list) or not plans or not isinstance(cases, list):
        raise DecisionError("Go trace plan or judgments lacks cases")
    judged = {}
    for row in cases:
        if not isinstance(row, dict):
            raise DecisionError("Go trace judgment case is malformed")
        judged[_name(row.get("task_id"), "Go trace judgment task ID")] = row
    if len(judged) != len(cases):
        raise DecisionError("Go trace judgments have duplicate or malformed task IDs")
    observed_tasks: set[str] = set()
    observed_repos: set[str] = set()
    tracked = {
        plan_path: hashlib.sha256(plan_raw).hexdigest(),
        judgments_path: hashlib.sha256(judgments_raw).hexdigest(),
    }
    rows = []
    for case in plans:
        if not isinstance(case, dict):
            raise DecisionError("Go trace plan case is malformed")
        repo = _name(case.get("repo_id"), "Go trace repository")
        task_ids = case.get("task_ids")
        if repo in observed_repos or not isinstance(task_ids, list) or len(task_ids) != 1:
            raise DecisionError("Go trace needs one unique task per repository")
        observed_repos.add(repo)
        task_id = _name(task_ids[0], "Go trace task ID")
        if task_id in observed_tasks or task_id not in judged:
            raise DecisionError("Go trace task is duplicate or lacks judgments")
        observed_tasks.add(task_id)
        judgment = judged[task_id]
        if judgment.get("source_commit") != case.get("commit") or judgment.get(
            "source_manifest_sha256"
        ) != case.get("manifest_sha256"):
            raise DecisionError(f"Go trace judgment source/manifest drift: {task_id}")
        if judgment.get("recall_denominator_exhaustive") is not True:
            raise DecisionError(f"Go trace task has incomplete qrels: {task_id}")
        gold = judgment.get("direct_production_gold")
        if not isinstance(gold, list) or not gold:
            raise DecisionError(f"Go trace task lacks exhaustive production gold: {task_id}")
        directory = _absolute_path(case.get("directory"), "Go trace case directory")
        if not run._is_hex(case.get("manifest_sha256"), 64) or not run._is_hex(
            case.get("pack_sha256"), 64
        ):
            raise DecisionError(f"Go trace case lacks pinned manifest or pack: {task_id}")

        def bound(name: str, digest: str | None = None, directory: Path = directory) -> dict:
            path = directory / name
            value, raw = read_strict(path, digest)
            if not isinstance(value, dict):
                raise DecisionError(f"Go trace {name} must be an object")
            tracked[path] = hashlib.sha256(raw).hexdigest()
            return value

        manifest = bound("manifest.json", case.get("manifest_sha256"))
        pack = bound("pack.json", case.get("pack_sha256"))
        if (
            manifest.get("repository_commit") != case.get("commit")
            or pack.get("repository_commit") != case.get("commit")
            or manifest.get("files") != pack.get("file_universe")
        ):
            raise DecisionError(f"Go trace corpus/pack drift: {task_id}")
        manifest_files = manifest.get("files")
        if not isinstance(manifest_files, list) or any(
            not isinstance(item, dict)
            or not isinstance(item.get("path"), str)
            or not run._is_hex(item.get("file_sha256"), 64)
            for item in manifest_files
        ):
            raise DecisionError(f"Go trace manifest file inventory is malformed: {task_id}")
        universe = {item["path"] for item in manifest_files}
        if len(universe) != len(manifest_files):
            raise DecisionError(f"Go trace manifest repeats a source file: {task_id}")
        gold_paths = [item.get("path") for item in gold if isinstance(item, dict)]
        if (
            len(gold_paths) != len(gold)
            or not all(isinstance(path, str) and path in universe for path in gold_paths)
            or len(set(gold_paths)) != len(gold_paths)
        ):
            raise DecisionError(f"Go trace gold lacks distinct source files: {task_id}")
        if reviewed:
            try:
                tracked.update(
                    go_trace_review_receipt.validate(
                        directory, case, judgment, denominator, manifest, pack
                    )
                )
            except (ValueError, OSError) as exc:
                raise DecisionError(f"Go trace source-only review invalid: {task_id}: {exc}") from exc
        execution = bound("execution.json")
        if (
            execution.get("status") != "VERIFIED"
            or execution.get("execution_source_head") != source_head
            or execution.get("policy_frozen_source_head") != source_head
            or execution.get("build_binding_sha256") != build_binding
        ):
            raise DecisionError(f"Go trace SDK attempt or policy source drift: {task_id}")
        if not run._is_hex(execution.get("paired_sha256"), 64):
            raise DecisionError(f"Go trace SDK attempt lacks paired capture: {task_id}")
        paired = bound("paired.json", execution["paired_sha256"])
        if (
            type(paired.get("schema_version")) is not int or paired["schema_version"] != 4
            or paired.get("generic_execution_profile_id")
            != query_plan.PROFILE_IDS["natural_language"]
        ):
            raise DecisionError(f"Go trace paired query profile drift: {task_id}")
        max_tokens = paired.get("nl_max_tokens")
        if type(max_tokens) is not int:
            raise DecisionError(f"Go trace paired natural-language config drift: {task_id}")
        nl_config = {**query_plan.DEFAULT_NL_CONFIG, "max_tokens": max_tokens}
        canonical_pack = benchmark_evidence.canonical_json(pack)
        if paired.get("pack_sha256") != hashlib.sha256(canonical_pack.encode()).hexdigest():
            raise DecisionError(f"Go trace paired pack identity drift: {task_id}")
        generation = paired.get("generation")
        if (
            not isinstance(generation, dict)
            or generation.get("repo_id") != repo
            or generation.get("revision_id") != case.get("commit")
            or type(generation.get("manifest_generation")) is not int
            or generation["manifest_generation"] <= 0
        ):
            raise DecisionError(f"Go trace paired generation drift: {task_id}")
        tasks = paired.get("tasks")
        pack_tasks = pack.get("tasks")
        if (
            not isinstance(tasks, list)
            or len(tasks) != 1
            or not isinstance(tasks[0], dict)
            or tasks[0].get("task_id") != task_id
            or not isinstance(pack_tasks, list)
            or len(pack_tasks) != 1
            or not isinstance(pack_tasks[0], dict)
            or pack_tasks[0].get("task_id") != task_id
        ):
            raise DecisionError(f"Go trace paired task drift: {task_id}")
        task = tasks[0]
        semantic_input = pack_tasks[0].get("semantic_input")
        identity = task.get("query_identity")
        raw_query = pack_tasks[0].get("query")
        if (
            not isinstance(raw_query, str)
            or pack_tasks[0].get("query_sha256")
            != hashlib.sha256(raw_query.encode()).hexdigest()
        ):
            raise DecisionError(f"Go trace query/pack identity drift: {task_id}")
        try:
            semantic_digest = query_plan.go_trace_semantic_digest(semantic_input)
            expected_identity = query_plan.derive_query_identity(
                "natural_language", raw_query, nl_config,
                semantic_input=semantic_input,
            )
        except query_plan.QueryPlanError as exc:
            raise DecisionError(f"Go trace query/pack identity drift: {task_id}: {exc}") from exc
        if (
            not isinstance(identity, dict)
            or set(identity) != {
                "original_query_sha256",
                "effective_lexical_request_sha256",
                "semantic_text_sha256",
                "generic_semantic_representation_policy",
                "trace_semantic_representation_policy",
                "trace_semantic_text_sha256",
            }
            or any(identity.get(key) != value for key, value in expected_identity.items())
            or identity.get("generic_semantic_representation_policy") != "task_query_v1"
            or identity.get("trace_semantic_representation_policy") != "go_test_excerpt_v1"
            or identity.get("trace_semantic_text_sha256") != semantic_digest
            or not run._is_hex(identity.get("effective_lexical_request_sha256"), 64)
        ):
            raise DecisionError(f"Go trace query/pack identity drift: {task_id}")
        trace = task.get("trace_files")
        context = trace.get("source_context") if isinstance(trace, dict) else None
        if (
            not isinstance(trace, dict)
            or trace.get("profile") != "go_trace_file_rrf_v2"
            or not isinstance(context, dict)
            or not isinstance(context.get("mode"), str)
            or context.get("mode") not in {
                "unowned", "explicit_pilot_owner_v1", "frozen_module_panic_v1"
            }
            or context.get("pilot_plan_sha256") != tracked[plan_path]
            or context.get("pilot_inputs_sha256") != plan.get("input_sha256")
            or context.get("manifest_sha256") != case.get("manifest_sha256")
            or context.get("pack_file_sha256") != case.get("pack_sha256")
            or trace.get("failure_excerpt_sha256") != semantic_input.get("failure_excerpt_sha256")
            or trace.get("semantic_representation_policy") != "go_test_excerpt_v1"
            or trace.get("semantic_text_sha256") != semantic_digest
        ):
            raise DecisionError(f"Go trace SDK source-context binding is missing: {task_id}")
        if (
            context["mode"] in {"explicit_pilot_owner_v1", "frozen_module_panic_v1"}
            and context.get("failure_source_repo_id") != repo
        ) or (context["mode"] == "unowned" and context.get("failure_source_repo_id") is not None):
            raise DecisionError(f"Go trace failure-source owner drift: {task_id}")
        relative_frame_path = None
        if context["mode"] == "frozen_module_panic_v1":
            if not reviewed:
                raise DecisionError(f"Go trace module frame requires reviewed holdout: {task_id}")
            try:
                tracked.update(
                    go_trace_review_receipt.validate_module_frame(
                        directory, case, plan, pack, manifest, context, trace
                    )
                )
            except (ValueError, OSError) as exc:
                raise DecisionError(f"Go trace module frame invalid: {task_id}: {exc}") from exc
            relative_frame_path = context["module_frame"]["path"]
        else:
            frame_paths = go_trace_review_receipt.relative_frame_paths(
                semantic_input["failure_excerpt"]
            )
            frame_class = ("unresolved", "exact", "ambiguous")[min(len(frame_paths), 2)]
            if trace.get("frame_class") != frame_class:
                raise DecisionError(f"Go trace frame class disagrees with failure excerpt: {task_id}")
            if frame_class == "exact" and context["mode"] == "explicit_pilot_owner_v1":
                relative_frame_path = next(iter(frame_paths))
        comparison = pack.get("comparison_contract")
        if (
            type(paired.get("lane_top_k")) is not int or paired["lane_top_k"] != 100
            or not isinstance(comparison, dict)
            or type(comparison.get("top_k")) is not int or comparison["top_k"] != 100
        ):
            raise DecisionError(f"Go trace SDK lane budget drift: {task_id}")

        f32_max = 3.4028234663852886e38

        def complete_window(
            value: object, where: str, count: int | None = None, task_id: str = task_id
        ) -> int:
            error = f"Go trace {where} window is incomplete: {task_id}"
            try:
                returned, exhausted, lane_execution = result_window.typed_window(value, where)
            except result_window.WindowEvidenceError as exc:
                raise DecisionError(f"{error}: {exc}") from exc
            expected_lanes = {
                "lexical": ("lexical",),
                "semantic": ("semantic.dense",),
                "hybrid": ("hybrid.lexical", "hybrid.dense"),
                "trace lexical": ("go_test_trace.lexical",),
                "trace dense": ("go_test_trace.dense",),
            }[where]
            if (
                returned > 100
                or (count is not None and returned != count)
                or any(lane not in lane_execution for lane in expected_lanes)
                or (returned > 0 and not any(lane_execution[lane] for lane in expected_lanes))
                or value.get("empty_provenance") == "logical_empty"
                or (
                    not exhausted
                    and (
                        returned != 100
                        or value["outcome"] != {"kind": "lower_bound", "continuation": True}
                    )
                )
            ):
                raise DecisionError(error)
            return returned

        def hybrid_rows(
            candidates: list, window: dict, task_id: str = task_id, f32_max: float = f32_max
        ) -> None:
            lanes_by_name = {lane["lane"]: lane for lane in window["coverage"]["lanes"]}
            lane_bounds = {}
            for lane, name in (("Lexical", "hybrid.lexical"), ("Dense", "hybrid.dense")):
                trace = lanes_by_name[name]
                lane_bounds[lane] = trace["candidates"].get("value", 0) if trace["contributed"] else 0
            seen_ranks = {lane: set() for lane in lane_bounds}
            seen_ids: set[str] = set()
            previous_order = None
            for candidate in candidates:
                if not isinstance(candidate, dict) or set(candidate) != {
                    "candidate_id", "source_repo_id", "path", "score", "contributions"
                }:
                    raise DecisionError(f"Go trace hybrid candidate provenance is invalid: {task_id}")
                candidate_id = candidate["candidate_id"]
                score = candidate["score"]
                contributions = candidate["contributions"]
                if (
                    not isinstance(candidate_id, str)
                    or not candidate_id
                    or candidate_id in seen_ids
                    or not is_finite_json_number(score)
                    or score <= 0
                    or not isinstance(contributions, list)
                    or not 1 <= len(contributions) <= 2
                ):
                    raise DecisionError(f"Go trace hybrid candidate provenance is invalid: {task_id}")
                seen_ids.add(candidate_id)
                lanes = []
                expected_score = 0.0
                for contribution in contributions:
                    if (
                        not isinstance(contribution, dict)
                        or set(contribution) != {"lane", "rank", "raw_score"}
                        or contribution["lane"] not in ("Lexical", "Dense")
                        or type(contribution["rank"]) is not int
                        or not 1 <= contribution["rank"] <= 0xFFFF_FFFF
                        or not is_finite_json_number(contribution["raw_score"])
                        or abs(contribution["raw_score"]) > f32_max
                    ):
                        raise DecisionError(f"Go trace hybrid lane provenance is invalid: {task_id}")
                    lane, rank = contribution["lane"], contribution["rank"]
                    if rank > lane_bounds[lane]:
                        raise DecisionError(f"Go trace hybrid lane rank exceeds the captured lane: {task_id}")
                    if rank in seen_ranks[lane]:
                        raise DecisionError(f"Go trace hybrid lane rank is repeated: {task_id}")
                    seen_ranks[lane].add(rank)
                    lanes.append(lane)
                    expected_score += 1.0 / (60.0 + rank)
                if lanes not in (["Lexical"], ["Dense"], ["Lexical", "Dense"]):
                    raise DecisionError(f"Go trace hybrid lane provenance is invalid: {task_id}")
                if score != expected_score:
                    raise DecisionError(f"Go trace hybrid RRF score disagrees with lane ranks: {task_id}")
                order = (-expected_score, "Lexical" not in lanes, candidate_id)
                if previous_order is not None and previous_order >= order:
                    raise DecisionError(f"Go trace hybrid ranking order is invalid: {task_id}")
                previous_order = order

        for route in ("lexical", "semantic", "hybrid"):
            capture = task.get(route)
            if not isinstance(capture, dict) or not isinstance(capture.get("candidates"), list):
                raise DecisionError(f"Go trace {route} capture is missing: {task_id}")
            complete_window(capture.get("window"), route, len(capture["candidates"]))
        trace_lane_returned = {
            lane: complete_window(trace.get(f"{lane}_window"), f"trace {lane}")
            for lane in ("lexical", "dense")
        }
        trace_page = _replay_go_trace_file_page(
            trace, repo, universe, trace_lane_returned, relative_frame_path, task_id
        )
        labels = [{"path": path, "grade": 1} for path in gold_paths]
        pages = {"trace_files": trace_page}
        for route in ("lexical", "hybrid"):
            candidates = task[route]["candidates"]
            if route == "hybrid":
                hybrid_rows(candidates, task[route]["window"])
            paths = {}
            for candidate in candidates:
                if not isinstance(candidate, dict):
                    raise DecisionError(f"Go trace {route} candidate is malformed: {task_id}")
                path = candidate.get("path")
                if (not isinstance(path, str) or path not in universe
                        or candidate.get("source_repo_id") != repo):
                    raise DecisionError(
                        f"Go trace {route} candidate lacks source provenance: {task_id}"
                    )
                paths.setdefault(path, {"path": path})
            pages[route] = list(paths.values())
        metrics = {
            route: {
                "Recall@20": evaluator.file_recall_at_k_judged(page, labels, 20),
                "MRR@20": evaluator.file_mrr_at_k_judged(page, labels, 20),
            }
            for route, page in pages.items()
        }
        rows.append({"repo_id": repo, "task_id": task_id, "metrics": metrics})
    if observed_tasks != set(judged):
        raise DecisionError("Go trace judgments and SDK task denominator differ")
    for path, digest in tracked.items():
        if hashlib.sha256(read_control(path)).hexdigest() != digest:
            raise DecisionError("Go trace inputs changed during replay")
    if out_path is not None:
        target = out_path.resolve()
        if (
            target in {path.resolve() for path in tracked}
            or out_path.exists()
            or out_path.is_symlink()
        ):
            raise DecisionError("Go trace output must be a new path outside its inputs")
    aggregate = {
        route: {
            metric: sum(row["metrics"][route][metric] for row in rows) / len(rows)
            for metric in ("Recall@20", "MRR@20")
        }
        for route in ("lexical", "hybrid", "trace_files")
    }
    return {
        "status": (
            "scored_complete_denominator_no_default_decision"
            if reviewed
            else "scored_exploratory_unqualified_no_default_decision"
        ),
        "product_default_decision": False,
        "review_receipts_verified": reviewed,
        "plan_sha256": tracked[plan_path],
        "judgments_sha256": tracked[judgments_path],
        "source_head": source_head,
        "denominator": len(rows),
        "metric_denominator": "exhaustive_positive_repository_tasks",
        "rows": rows,
        "aggregate": aggregate,
    }


def build_native_file_decision(
    manifest: Path,
    policy: Path,
    daemon: Path,
    config: Path,
    runner: Path,
    *,
    execution_evidence_root: Path | None = None,
    execution_run_ids: list[str] | None = None,
) -> dict:
    """Keep Native execution evidence separate from Legacy run qualification."""
    from tools.benchmark.retrieval.native_file_decision import build_native_file_decision as replay

    options = (
        {}
        if execution_evidence_root is None and execution_run_ids is None
        else {
            "execution_evidence_root": execution_evidence_root,
            "execution_run_ids": execution_run_ids,
        }
    )
    return replay(manifest, policy, daemon, config, runner, **options)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for key in (
        "repo",
        "suite",
        "run-manifest",
        "policy",
        "out",
        "go-trace-plan",
        "go-trace-judgments",
        "native-file-cohort",
        "daemon-binary",
        "daemon-config",
        "runner-binary",
        "native-execution-root",
        "native-execution-runs",
    ):
        parser.add_argument(f"--{key}")
    args = parser.parse_args(argv)
    try:
        native = any(
            (
                args.native_file_cohort,
                args.daemon_binary,
                args.daemon_config,
                args.runner_binary,
                args.native_execution_root,
                args.native_execution_runs,
            )
        )
        if native:
            if not all(
                (
                    args.native_file_cohort,
                    args.policy,
                    args.daemon_binary,
                    args.daemon_config,
                    args.runner_binary,
                    args.out,
                )
            ) or any(
                (
                    args.repo,
                    args.suite,
                    args.run_manifest,
                    args.go_trace_plan,
                    args.go_trace_judgments,
                )
            ):
                raise DecisionError(
                    "Native decision requires only cohort, policy, daemon/config, runner and out"
                )
            output = Path(args.out)
            if output.exists() or output.is_symlink():
                raise DecisionError("Native decision output must be a new path")
            if (args.native_execution_root is None) != (args.native_execution_runs is None):
                raise DecisionError("Native execution evidence needs both root and run IDs")
            options = {}
            if args.native_execution_root is not None:
                options = {
                    "execution_evidence_root": Path(args.native_execution_root),
                    "execution_run_ids": parse_json(
                        read_control(Path(args.native_execution_runs)).decode("utf-8")
                    ),
                }
            decision = build_native_file_decision(
                Path(args.native_file_cohort),
                Path(args.policy),
                Path(args.daemon_binary),
                Path(args.daemon_config),
                Path(args.runner_binary),
                **options,
            )
            rendered = json.dumps(decision, indent=2, sort_keys=True, allow_nan=False) + "\n"
            with output.open("x", encoding="utf-8") as stream:
                stream.write(rendered)
        elif args.go_trace_plan or args.go_trace_judgments:
            if (
                not args.go_trace_plan
                or not args.go_trace_judgments
                or not args.out
                or any((args.repo, args.suite, args.run_manifest, args.policy))
            ):
                raise DecisionError("Go trace evaluation needs only plan, judgments and out")
            decision = evaluate_go_trace_file_holdout(
                Path(args.go_trace_plan), Path(args.go_trace_judgments), Path(args.out)
            )
        else:
            if not all((args.repo, args.suite, args.run_manifest, args.policy, args.out)):
                raise DecisionError(
                    "default decision requires repo, suite, run-manifest, policy and out"
                )
            decision = build_decision(
                Path(args.repo), Path(args.suite), Path(args.run_manifest), Path(args.policy)
            )
    except (DecisionError, run.RunError, ValueError, OSError, KeyError, TypeError) as exc:
        scope = (
            "Native quality decision"
            if native
            else (
                "Go trace evaluation"
                if args.go_trace_plan or args.go_trace_judgments
                else "default decision"
            )
        )
        print(f"ERROR: {scope} refused: {exc}", file=sys.stderr)
        return 2
    if args.go_trace_plan and not native:
        with Path(args.out).open("x", encoding="utf-8") as output:
            output.write(json.dumps(decision, indent=2, sort_keys=True) + "\n")
    elif not native:
        Path(args.out).write_text(
            json.dumps(decision, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    print(json.dumps({"status": decision["status"], "reasons": decision.get("reasons", [])}))
    return (
        0
        if decision["status"]
        in {
            "admit",
            "quality_admit",
            "scored_complete_denominator_no_default_decision",
            "scored_exploratory_unqualified_no_default_decision",
        }
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())
