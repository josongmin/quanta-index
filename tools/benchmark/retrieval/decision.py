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
from tools.benchmark.retrieval import evaluator, run
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
        "repository-disjoint decision policy",
    )
    if type(policy["schema_version"]) is not int or policy["schema_version"] != 2:
        raise DecisionError("repository-disjoint policy schema_version must be 2")
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
    single = {
        **policy,
        "schema_version": 1,
        "repository_scope": {
            "kind": "single_repository",
            "repository_commit": holdout[0]["repository_commit"],
        },
        "confidence_method": "paired_query_family_cluster_bootstrap_percentile_v1",
    }
    validate_policy(single)
    if (
        policy["confidence_method"]
        != "paired_stratified_repository_cluster_bootstrap_percentile_v1"
    ):
        raise DecisionError("repository-disjoint uncertainty method is unsupported")
    required_repo_strata = {(row["axis"], row["name"]) for row in policy["critical_strata"]}
    if any(("repository", name) not in required_repo_strata for name in names):
        raise DecisionError("repository-disjoint critical strata omit a holdout repository")
    return policy


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
            or not set(row["query_family_ids"]).issubset(source["query_family_ids"])
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
        manifest = run._validate_manifest_shape(bound_json(manifest_path))
        if manifest["scope"] != "qualified":
            raise DecisionError("repository-disjoint capture is not qualified")
        root = manifest_path.resolve().parent
        admission_path = run._resolve_artifact(
            root, manifest["artifacts"]["admission_manifest"], "admission"
        )
        admission = run.validate_admission_manifest(bound_json(admission_path))
        if (
            admission.get("decision_policy_sha256") != policy_sha
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
        if (
            not isinstance(observed, dict)
            or observed.get("primary_metric") != comparison["primary_metric"]
            or observed.get("primary_delta") != selected[0].get("primary_delta")
            or observed.get("sample_count") != selected[0].get("sample_count")
        ):
            raise DecisionError("repository-disjoint report metric differs from policy")
        suites[name], reports[name] = suite, report
        receipts.append(
            {
                "repository": name,
                "admission_sha256": initial[admission_path],
                "report_sha256": selected[0]["report_digest"],
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


def _candidate_measurements(
    manifest: dict, paths: dict, comparison: dict, bound_json
) -> dict:
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
