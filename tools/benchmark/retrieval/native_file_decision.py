"""Quality gate for replayed Native file captures and their input commitments.

This module does not authorize a product default. Installed provenance and the
open-loop operational profile have separate acceptance contracts.
"""

from __future__ import annotations

import hashlib
import math
import stat
from pathlib import Path

from tools.benchmark.evidence import file_digest, read_control
from tools.benchmark.retrieval import evaluator, native_file_evidence
from tools.benchmark.retrieval.finite_json import is_finite_json_number


class NativeDecisionError(ValueError):
    """A Native quality decision input is malformed or lacks authority."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise NativeDecisionError(message)


def _sha(path: Path) -> str:
    return file_digest(path.resolve())[0].removeprefix("sha256:")


def _release_tree_sha(root: Path) -> str:
    """Bind the complete frozen release directory, including names and modes."""
    import sys

    _require(root.is_dir(), "Native release input must be a directory")
    # The canonical release validator owns the symlink/special-file tree rule.
    benchmark_root = str(Path(evaluator.__file__).resolve().parents[1])
    if benchmark_root not in sys.path:
        sys.path.insert(0, benchmark_root)
    import corpus_release

    rows = [
        {
            "path": name,
            "digest": _sha(root / name),
            "mode": stat.S_IMODE((root / name).stat().st_mode),
        }
        for name in sorted(corpus_release.regular_tree(root))
    ]
    _require(rows, "Native release directory is empty")
    return hashlib.sha256(evaluator.canonical(rows)).hexdigest()


def _policy(path: Path, manifest_sha: str, manifest: dict) -> tuple[dict, str]:
    raw = read_control(path.resolve())
    value = evaluator._parse_json_text(raw.decode("utf-8"))
    _require(
        isinstance(value, dict)
        and set(value)
        == {
            "schema_version",
            "profile",
            "cohort_manifest_sha256",
            "repositories",
            "repository_strata",
            "minimum_counts",
            "no_answer_rule",
            "resource_limits",
        }
        and type(value["schema_version"]) is int
        and value["schema_version"] == 1
        and value["profile"] == "native_file_decision"
        and value["cohort_manifest_sha256"] == manifest_sha
        and value["repositories"] == manifest["repositories"]
        and value["repository_strata"] == manifest["repository_strata"]
        and value["no_answer_rule"] == "nonempty_top10_is_false_positive",
        "Native decision policy or frozen cohort differs",
    )
    minima = value["minimum_counts"]
    _require(
        isinstance(minima, dict)
        and set(minima) == {"positive", "identifier", "negative", "hard_negative"}
        and all(
            type(minima[key]) is int and minima[key] >= floor
            for key, floor in (
                ("positive", 100),
                ("identifier", 30),
                ("negative", 30),
                ("hard_negative", 30),
            )
        ),
        "Native decision minimum_counts must be frozen integer counts at or above the DOD floors",
    )
    limits = value["resource_limits"]
    _require(
        isinstance(limits, dict)
        and set(limits) == {"max_query_p95_micros"}
        and is_finite_json_number(limits["max_query_p95_micros"])
        and 0 < limits["max_query_p95_micros"] <= 100_000,
        "Native query p95 limit must be predeclared and no weaker than 100 ms",
    )
    return value, hashlib.sha256(raw).hexdigest()


_RUNTIME_KEYS = {
    "schema_version",
    "kind",
    "daemon_binary_sha256",
    "daemon_config_sha256",
    "runner_binary_sha256",
    "decision_policy_sha256",
    "socket_path",
    "config_path",
    "command",
    "pid",
    "uid",
    "gid",
    "peer_image_verified",
}


def _check_runtime(
    capture: dict,
    *,
    daemon_sha: str,
    config_sha: str,
    runner_sha: str,
    policy_sha: str,
    config_path: Path,
    socket: str,
) -> None:
    binding = capture.get("runtime_binding")
    _require(
        isinstance(binding, dict)
        and set(binding) == _RUNTIME_KEYS
        and type(binding["schema_version"]) is int
        and binding["schema_version"] == 1
        and binding["kind"] == "linux_connected_peer_image_v1"
        and binding["daemon_binary_sha256"] == daemon_sha
        and binding["daemon_config_sha256"] == config_sha
        and binding["runner_binary_sha256"] == runner_sha
        and binding["decision_policy_sha256"] == policy_sha
        and binding["socket_path"] == socket
        and binding["config_path"] == str(config_path)
        and binding["command"] in {"serve-native", "serve-native-candidate"}
        and type(binding["pid"]) is int
        and binding["pid"] > 0
        and type(binding["uid"]) is int
        and binding["uid"] >= 0
        and type(binding["gid"]) is int
        and binding["gid"] >= 0
        and binding["peer_image_verified"] is True
        and capture.get("runner_binary_sha256") == runner_sha,
        "Native capture is not bound to the connected daemon, config, runner and policy",
    )


def _score_pair(rows: dict, task_id: str, field: str) -> float:
    before = rows.get((task_id, "lexical"), {}).get(field)
    after = rows.get((task_id, "hybrid"), {}).get(field)
    _require(
        is_finite_json_number(before)
        and is_finite_json_number(after)
        and 0 <= before <= 1
        and 0 <= after <= 1,
        f"Native paired {field} missing or outside [0,1]: {task_id}",
    )
    return float(after - before)


def _slice_mean(rows: list[tuple[str, str, str, float]]) -> float:
    """Equal family weight within each repository, then equal repository weight."""
    grouped: dict[str, dict[str, list[float]]] = {}
    for repo, _task, family, delta in rows:
        grouped.setdefault(repo, {}).setdefault(family, []).append(delta)
    repository_means = [
        math.fsum(math.fsum(values) / len(values) for values in families.values()) / len(families)
        for families in grouped.values()
    ]
    return math.fsum(repository_means) / len(repository_means)


def _nearest_rank(values: list[int], fraction: float) -> int:
    ordered = sorted(values)
    return ordered[math.ceil(fraction * len(ordered)) - 1]


def verify_controlled_capture_runs(
    manifest_path: Path,
    policy_path: Path,
    daemon_binary: Path,
    daemon_config: Path,
    runner_binary: Path,
    evidence_root: Path,
    run_ids: list[str],
) -> dict[str, str]:
    """Return the validated capture commitment for each owned family run."""
    import sys

    benchmark_root = str(Path(evaluator.__file__).resolve().parents[1])
    if benchmark_root not in sys.path:
        sys.path.insert(0, benchmark_root)
    import native_file_capture
    from evidence import RunStore, file_digest
    from evidence_bridge import source_identity
    from profile_capture import load_capture
    from registry import load_registry, registry_digest

    manifest_path = manifest_path.resolve()
    _raw, manifest = evaluator._native_cohort_manifest(manifest_path)
    expected = {entry["family_id"] for entry in manifest["families"]}
    _require(
        isinstance(run_ids, list)
        and len(run_ids) == len(expected)
        and len(set(run_ids)) == len(run_ids),
        "Native controlled run inventory is missing or repeated",
    )
    code_root = Path(__file__).resolve().parents[3]
    current_source = source_identity(code_root, "benchmark-retrieval")
    store = RunStore(evidence_root)
    registry_sha = registry_digest(load_registry())
    commitments: dict[str, str] = {}
    expected_paths = {
        "cohort_manifest": manifest_path,
        "decision_policy": policy_path.resolve(),
        "daemon_binary": daemon_binary.resolve(),
        "daemon_config": daemon_config.resolve(),
        "runner_binary": runner_binary.resolve(),
    }
    for run_id in run_ids:
        suffix = "-" + native_file_capture.FAMILY
        _require(
            isinstance(run_id, str) and run_id.endswith(suffix),
            "Native controlled run ID has the wrong family",
        )
        capture_id = run_id[: -len(suffix)]
        document = load_capture(
            evidence_root,
            profile=native_file_capture.PROFILE,
            registry_digest=registry_sha,
            capture_id=capture_id,
        )
        _require(
            [row["run_id"] for row in document["runs"]] == [run_id],
            "Native controlled run lacks sole original capture membership",
        )
        evidence = store.load(run_id)
        _require(
            evidence["source"] == current_source,
            "Native controlled capture code source is stale",
        )
        _require(
            evidence["build"]["lockfile_digest"] == file_digest(code_root / "Cargo.lock")[0],
            "Native controlled capture lockfile is stale",
        )
        commitment = native_file_capture.replay_run(store, evidence)
        plan = native_file_capture._plan(store.run_dir(run_id) / "raw" / "plan.json")
        _require(
            all(Path(plan[key]).resolve() == path for key, path in expected_paths.items()),
            "Native controlled capture belongs to a different input cohort",
        )
        commitments[evidence["case_id"]] = commitment
    _require(commitments.keys() == expected, "Native controlled capture family coverage differs")
    return commitments


def build_native_file_decision(
    manifest_path: Path,
    policy_path: Path,
    daemon_binary: Path,
    daemon_config: Path,
    runner_binary: Path,
    *,
    execution_evidence_root: Path | None = None,
    execution_run_ids: list[str] | None = None,
) -> dict:
    """Replay all raw Native families and apply fixed quality thresholds.

    Capture fields are compared with pinned files. They do not independently
    prove runner execution, installation or the 960-second production profile.
    """
    _require(
        (execution_evidence_root is None) == (execution_run_ids is None),
        "Native controlled execution needs both evidence root and run IDs",
    )
    # Bind relative inputs to the same resolved manifest location we consume.
    # A top-level alias (for example /tmp on macOS) is not a release-tree link.
    manifest_path = manifest_path.resolve()
    raw_manifest, manifest = evaluator._native_cohort_manifest(manifest_path)
    manifest_sha = hashlib.sha256(raw_manifest).hexdigest()
    policy, policy_sha = _policy(policy_path, manifest_sha, manifest)
    daemon_sha, config_sha, runner_sha = (
        _sha(daemon_binary),
        _sha(daemon_config),
        _sha(runner_binary),
    )
    config = evaluator.read_json(daemon_config)
    _require(
        isinstance(config, dict)
        and isinstance(config.get("socket_path"), str)
        and Path(config["socket_path"]).is_absolute(),
        "Native daemon config lacks an absolute socket_path",
    )
    socket = config["socket_path"]
    config_path = daemon_config.resolve()
    tracked_files = {
        manifest_path: manifest_sha,
        policy_path: policy_sha,
        daemon_binary: daemon_sha,
        daemon_config: config_sha,
        runner_binary: runner_sha,
    }

    def path(entry: dict, key: str) -> Path:
        return (manifest_path.parent / entry[key]).resolve()

    def release_path(entry: dict, key: str) -> Path:
        # Keep the lexical path so the canonical release validator sees aliases.
        return (manifest_path.parent / entry[key]).absolute()

    release_paths = {
        release_path(entry, key)
        for entry in manifest["families"]
        for key in ("base_release", "target_releases")
    }
    tracked_releases = {root: _release_tree_sha(root) for root in release_paths}
    file_keys = ("suite", "query_pack", "capture", "source_universe", "source_binding")
    for entry in manifest["families"]:
        for key in file_keys:
            source = path(entry, key)
            observed = _sha(source)
            _require(
                source not in tracked_files or tracked_files[source] == observed,
                "Native decision input path is reused with different bytes",
            )
            tracked_files[source] = observed

    families = []
    for entry in manifest["families"]:
        for key in file_keys:
            source = path(entry, key)
            _require(
                tracked_files[source] == _sha(source),
                "Native decision input changed before family replay",
            )
        replay = evaluator.load_native_file_replay(
            path(entry, "repo"),
            path(entry, "suite"),
            path(entry, "query_pack"),
            path(entry, "capture"),
            (
                path(entry, "source_universe"),
                path(entry, "source_binding"),
                release_path(entry, "base_release"),
                release_path(entry, "target_releases"),
                entry["family_id"],
            ),
        )
        _check_runtime(
            replay["capture"],
            daemon_sha=daemon_sha,
            config_sha=config_sha,
            runner_sha=runner_sha,
            policy_sha=policy_sha,
            config_path=config_path,
            socket=socket,
        )
        families.append(replay)
    # This existing gate checks the complete source and target roster, family
    # identities, captures, replay commitments and independent MRR rows.
    cohort = native_file_evidence.repository_cluster_ci_from_native_families(
        families, manifest["repositories"], manifest["repository_strata"]
    )
    positive: list[tuple[str, str, str, float]] = []
    recall: list[tuple[str, str, str, float]] = []
    identifier: list[tuple[str, str, str, float]] = []
    negative: list[tuple[str, str, str, float]] = []
    hard_negative: list[tuple[str, str, str, float]] = []
    timings: list[int] = []
    for entry, replay in zip(manifest["families"], families, strict=True):
        suite, capture, report = (replay[key] for key in ("suite", "capture", "report"))
        family = entry["family_id"]
        target = report["composite_source_binding"]["target_repository"]
        _require(family == report["composite_source_binding"]["family_id"], "Native family differs")
        by_key = {(row["task_id"], row["route"]): row for row in report["per_query"]}
        _require(len(by_key) == len(report["per_query"]), "Native query pair is duplicated")
        captured = {row["task_id"]: row for row in capture["tasks"]}
        _require(len(captured) == len(capture["tasks"]), "Native timed task is duplicated")
        for task in suite["tasks"]:
            if task["split"] != "eval":
                continue
            task_id = task["task_id"]
            timing = captured[task_id].get("hybrid_micros")
            _require(
                type(timing) is int and 0 <= timing <= 60_000_000, "Native hybrid timing is invalid"
            )
            timings.append(timing)
            if task["answerable"]:
                mrr = _score_pair(by_key, task_id, "file_mrr_at_10")
                rec = _score_pair(by_key, task_id, "file_recall_at_10")
                positive.append((target, task_id, family, mrr))
                recall.append((target, task_id, family, rec))
                if task.get("category") == "identifier":
                    identifier.append((target, task_id, family, mrr))
            else:
                before = by_key.get((task_id, "lexical"), {}).get("status")
                after = by_key.get((task_id, "hybrid"), {}).get("status")
                _require(
                    before in {"abstained", "success"} and after in {"abstained", "success"},
                    "Native negative task lacks an observed abstention status",
                )
                delta = float(after == "success") - float(before == "success")
                negative.append((target, task_id, family, delta))
                if task.get("category") == "hard_negative":
                    hard_negative.append((target, task_id, family, delta))
    _require(timings and positive, "Native quality cohort has no timed positive tasks")
    primary_ci = evaluator.repository_cluster_ci(
        positive,
        cohort["cohort_sha256"],
        manifest["repositories"],
        manifest["repository_strata"],
    )
    recall_ci = evaluator.repository_cluster_ci(
        recall,
        cohort["cohort_sha256"],
        manifest["repositories"],
        manifest["repository_strata"],
    )
    counts = {
        "positive": len(positive),
        "identifier": len(identifier),
        "negative": len(negative),
        "hard_negative": len(hard_negative),
        "repositories": len(manifest["repositories"]),
        "identifier_repositories": len({row[0] for row in identifier}),
        "negative_repositories": len({row[0] for row in negative}),
        "hard_negative_repositories": len({row[0] for row in hard_negative}),
    }
    identifier_mean = _slice_mean(identifier) if identifier else None
    negative_mean = _slice_mean(negative) if negative else None
    hard_negative_mean = _slice_mean(hard_negative) if hard_negative else None
    p95, p99 = _nearest_rank(timings, 0.95), _nearest_rank(timings, 0.99)
    reasons = []
    if counts["positive"] < policy["minimum_counts"]["positive"]:
        reasons.append("positive_denominator_below_frozen_minimum")
    if primary_ci["status"] != "available" or recall_ci["status"] != "available":
        reasons.append("repository_ci_unavailable")
    elif primary_ci["mean"] < 0.02 or primary_ci["lower_95"] <= 0:
        reasons.append("mrr_gain_or_lower_bound_failed")
    if recall_ci["status"] == "available" and recall_ci["lower_95"] < -0.01:
        reasons.append("recall_lower_bound_failed")
    if (
        counts["identifier"] < policy["minimum_counts"]["identifier"]
        or counts["identifier_repositories"] < 4
    ):
        reasons.append("identifier_denominator_insufficient")
    elif identifier_mean < -0.01:
        reasons.append("identifier_mrr_regressed")
    if (
        counts["negative"] < policy["minimum_counts"]["negative"]
        or counts["negative_repositories"] < 4
    ):
        reasons.append("negative_denominator_insufficient")
    elif negative_mean > 0.01:
        reasons.append("negative_false_positive_regressed")
    if (
        counts["hard_negative"] < policy["minimum_counts"]["hard_negative"]
        or counts["hard_negative_repositories"] < 4
    ):
        reasons.append("hard_negative_denominator_insufficient")
    elif hard_negative_mean > 0.01:
        reasons.append("hard_negative_false_positive_regressed")
    if p95 > policy["resource_limits"]["max_query_p95_micros"] or p99 > 500_000:
        reasons.append("observed_query_latency_exceeded")
    controlled_capture_verified = False
    if execution_evidence_root is not None:
        controlled = verify_controlled_capture_runs(
            manifest_path,
            policy_path,
            daemon_binary,
            daemon_config,
            runner_binary,
            execution_evidence_root,
            execution_run_ids,
        )
        scored = {
            entry["family_id"]: replay["report"]["capture_commitment_sha256"]
            for entry, replay in zip(manifest["families"], families, strict=True)
        }
        _require(controlled == scored, "Native controlled execution differs from scored capture")
        controlled_capture_verified = True
    _require(
        all(_sha(path) == sha for path, sha in tracked_files.items())
        and all(_release_tree_sha(path) == sha for path, sha in tracked_releases.items()),
        "Native decision inputs changed during replay",
    )
    return {
        "status": "quality_reject" if reasons else "quality_admit",
        "reasons": reasons,
        "product_default_decision": False,
        "runtime_qualified": False,
        "execution_binding_verified": False,
        "controlled_capture_verified": controlled_capture_verified,
        "controlled_capture_scope": "owned_subprocess_record_and_replayed_raw_capture_only",
        "capture_binding_consistent": True,
        "capture_consistency_scope": "replayed_capture_commitments_and_current_input_files",
        "qualification_scope": "replayed_file_quality_and_capture_consistency_only",
        "cohort_manifest_sha256": manifest_sha,
        "decision_policy_sha256": policy_sha,
        "cohort_sha256": cohort["cohort_sha256"],
        "minimum_counts": policy["minimum_counts"],
        "counts": counts,
        "metrics": {
            "mrr": {"repository_cluster_ci": primary_ci},
            "recall": {"repository_cluster_ci": recall_ci},
            "identifier_mrr_delta": identifier_mean,
            "negative_false_positive_delta": negative_mean,
            "hard_negative_false_positive_delta": hard_negative_mean,
            "query_p95_micros": p95,
            "query_p99_micros": p99,
        },
    }
