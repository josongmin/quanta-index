"""Quality-gate units with substituted replay and real release-directory shapes.

The raw Git/suite/pack/capture loader is tested in test_native_file_evidence.py;
these gate fixtures do not prove SDK execution or corpus-release admission.
"""

from __future__ import annotations

import copy
import hashlib
import json
import os
import shutil
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import native_file_decision as decision
from tools.ci.tests.test_native_file_evidence import _replayed_native_family


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _case(tmp_path: Path) -> tuple[Path, Path, list[Path], list[dict]]:
    daemon, config, runner = [tmp_path / name for name in ("daemon", "config.json", "runner")]
    daemon.write_bytes(b"native daemon image")
    config.write_text(json.dumps({"socket_path": str(tmp_path / "native.sock")}), encoding="utf-8")
    runner.write_bytes(b"native file capture image")
    families = []
    repositories = {f"repo{i:02d}": f"{i + 1:040x}" for i in range(12)}
    strata = {name: ("a" if i < 6 else "b") for i, name in enumerate(repositories)}
    for i in range(12):
        item = _replayed_native_family(i, f"family-{i}", [0.10] * 13)
        suite, capture, report = (item[key] for key in ("suite", "capture", "report"))
        for j, task in enumerate(suite["tasks"]):
            if j < 3:
                task["category"] = "identifier"
            if j >= 10:
                task["answerable"] = False
                task["category"] = "hard_negative"
            capture["tasks"][j]["lexical_micros"] = 3000
            capture["tasks"][j]["hybrid_micros"] = 4000
            for row in report["per_query"][2 * j : 2 * j + 2]:
                row["file_recall_at_10"] = 0.5 if row["route"] == "lexical" else 0.6
                if j >= 10:
                    row["file_mrr_at_10"] = 0.0
                    row["status"] = "abstained"
        report["rank_metrics"]["comparison"]["sample_count"] = 10
        report["rank_metrics"]["comparison"]["primary_delta"] = 0.1
        capture["runtime_binding"] = {
            "schema_version": 1,
            "kind": "linux_connected_peer_image_v1",
            "daemon_binary_sha256": _sha(daemon),
            "daemon_config_sha256": _sha(config),
            "runner_binary_sha256": _sha(runner),
            "decision_policy_sha256": "",
            "socket_path": str(tmp_path / "native.sock"),
            "config_path": str(config),
            "command": "serve-native-candidate",
            "pid": 100 + i,
            "uid": 501,
            "gid": 20,
            "peer_image_verified": True,
        }
        capture["runner_binary_sha256"] = _sha(runner)
        report["suite_commitment_sha256"] = ev.digest(ev.canonical(suite))
        capture["suite_commitment_sha256"] = report["suite_commitment_sha256"]
        families.append(item)
    manifest = {
        "schema_version": 1,
        "profile": "native_file_family_cohort",
        "repositories": repositories,
        "repository_strata": strata,
        "families": [
            {
                key: f"inputs/{i}/{key}"
                if key in ("base_release", "target_releases")
                else f"inputs/{i}/{key}.json"
                for key in (
                    "repo",
                    "suite",
                    "query_pack",
                    "capture",
                    "source_universe",
                    "source_binding",
                    "base_release",
                    "target_releases",
                )
            }
            | {"family_id": f"family-{i}"}
            for i in range(12)
        ],
    }
    manifest_path = tmp_path / "cohort.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    for family in manifest["families"]:
        for key in (
            "suite",
            "query_pack",
            "capture",
            "source_universe",
            "source_binding",
            "base_release",
            "target_releases",
        ):
            artifact = tmp_path / family[key]
            if key in ("base_release", "target_releases"):
                artifact.mkdir(parents=True)
                release = artifact if key == "base_release" else artifact / "target"
                release.mkdir(exist_ok=True)
                (release / "release.json").write_text("{}", encoding="utf-8")
            else:
                artifact.parent.mkdir(parents=True, exist_ok=True)
                artifact.write_text("{}", encoding="utf-8")
    policy = {
        "schema_version": 1,
        "profile": "native_file_decision",
        "cohort_manifest_sha256": _sha(manifest_path),
        "repositories": repositories,
        "repository_strata": strata,
        "minimum_counts": {
            "positive": 100,
            "identifier": 30,
            "negative": 30,
            "hard_negative": 30,
        },
        "no_answer_rule": "nonempty_top10_is_false_positive",
        "resource_limits": {"max_query_p95_micros": 100000},
    }
    policy_path = tmp_path / "policy.json"
    policy_path.write_text(json.dumps(policy), encoding="utf-8")
    for item in families:
        item["capture"]["runtime_binding"]["decision_policy_sha256"] = _sha(policy_path)
        item["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(item["capture"]))
    return manifest_path, policy_path, [daemon, config, runner], families


def test_native_quality_uses_repository_equal_ci_and_keeps_product_default_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *args: copy.deepcopy(by_family[args[-1][-1]])
    )
    result = decision.build_native_file_decision(manifest, policy, *binaries)
    assert result["status"] == "quality_admit"
    assert result["controlled_capture_verified"] is False
    assert result["product_default_decision"] is False
    assert result["execution_binding_verified"] is False
    assert result["capture_binding_consistent"] is True
    assert result["metrics"]["mrr"]["repository_cluster_ci"]["mean"] == pytest.approx(0.1)
    assert result["counts"]["positive"] == 120
    assert result["counts"]["identifier"] == 36
    assert result["counts"]["negative"] == 36


def test_native_quality_resolves_manifest_location_before_relative_release_inputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    alias = tmp_path.parent / (tmp_path.name + "-alias")
    alias.symlink_to(tmp_path, target_is_directory=True)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *args: copy.deepcopy(by_family[args[-1][-1]])
    )
    result = decision.build_native_file_decision(alias / manifest.name, policy, *binaries)
    assert result["status"] == "quality_admit"
    assert result["execution_binding_verified"] is False


@pytest.mark.parametrize(
    "mutation",
    [
        "stale_daemon",
        "stale_policy",
        "missing_runtime",
        "false_peer",
        "bad_recall",
        "bad_identifier",
        "bad_negative",
        "short_denominator",
        "slow_query",
    ],
)
def test_native_quality_refuses_or_rejects_broken_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    if mutation == "stale_daemon":
        binaries[0].write_bytes(b"different daemon")
    elif mutation == "stale_policy":
        policy.write_text(policy.read_text() + " ", encoding="utf-8")
    elif mutation == "missing_runtime":
        del families[0]["capture"]["runtime_binding"]
    elif mutation == "false_peer":
        families[0]["capture"]["runtime_binding"]["peer_image_verified"] = False
    elif mutation == "bad_recall":
        for item in families:
            for row in item["report"]["per_query"]:
                if row["route"] == "hybrid":
                    row["file_recall_at_10"] = 0.0
    elif mutation == "bad_identifier":
        for item in families:
            for row in item["report"]["per_query"][:6]:
                row["file_mrr_at_10"] = 0.5 if row["route"] == "lexical" else 0.0
            item["report"]["rank_metrics"]["comparison"]["primary_delta"] = -0.08
    elif mutation == "bad_negative":
        for item in families:
            for row in item["report"]["per_query"][20:]:
                if row["route"] == "hybrid":
                    row["status"] = "success"
    elif mutation == "short_denominator":
        for item in families:
            for task in item["suite"]["tasks"][2:10]:
                task["answerable"] = False
                for row in item["report"]["per_query"]:
                    if row["task_id"] == task["task_id"]:
                        row["status"] = "abstained"
                        row["file_mrr_at_10"] = 0.0
            item["report"]["rank_metrics"]["comparison"]["sample_count"] = 2
            item["report"]["rank_metrics"]["comparison"]["primary_delta"] = 0.1
            suite_sha = ev.digest(ev.canonical(item["suite"]))
            item["capture"]["suite_commitment_sha256"] = suite_sha
            item["report"]["suite_commitment_sha256"] = suite_sha
            item["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(item["capture"]))
    else:
        for item in families:
            for task in item["capture"]["tasks"]:
                task["hybrid_micros"] = 200000
            item["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(item["capture"]))
    by_family = {item["suite"]["suite_id"]: item for item in families}
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *args: copy.deepcopy(by_family[args[-1][-1]])
    )
    if mutation in {
        "stale_daemon",
        "stale_policy",
        "missing_runtime",
        "false_peer",
    }:
        with pytest.raises((ev.EvidenceError, decision.NativeDecisionError)):
            decision.build_native_file_decision(manifest, policy, *binaries)
    else:
        result = decision.build_native_file_decision(manifest, policy, *binaries)
        assert result["status"] == "quality_reject"
        assert result["product_default_decision"] is False
        if mutation == "short_denominator":
            assert "positive_denominator_below_frozen_minimum" in result["reasons"]


def test_native_quality_refuses_input_changed_during_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    changed = tmp_path / "inputs/0/capture.json"

    def replay(*args: object) -> dict:
        changed.write_text('{"changed":true}', encoding="utf-8")
        return copy.deepcopy(by_family[args[-1][-1]])

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    with pytest.raises(decision.NativeDecisionError, match="changed"):
        decision.build_native_file_decision(manifest, policy, *binaries)


@pytest.mark.parametrize("change_on_disk", [False, True])
def test_controlled_capture_must_match_scored_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change_on_disk: bool
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *args: copy.deepcopy(by_family[args[-1][-1]])
    )
    capture_paths = {
        f"family-{i}": tmp_path / f"inputs/{i}/capture.json" for i in range(len(families))
    }
    scored = {
        family: item["report"]["capture_commitment_sha256"] for family, item in by_family.items()
    }

    def verify(*_args: object) -> dict[str, str]:
        if change_on_disk:
            capture_paths["family-0"].write_text('{"replacement":true}', encoding="utf-8")
            return scored
        return {**scored, "family-0": hashlib.sha256(b"other capture").hexdigest()}

    monkeypatch.setattr(decision, "verify_controlled_capture_runs", verify)
    with pytest.raises(decision.NativeDecisionError, match="changed|scored capture"):
        decision.build_native_file_decision(
            manifest,
            policy,
            *binaries,
            execution_evidence_root=tmp_path,
            execution_run_ids=["fixture"],
        )


@pytest.mark.parametrize("mutation", ["base_content", "target_membership", "target_mode"])
def test_native_quality_refuses_release_tree_changed_during_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    base = tmp_path / "inputs/0/base_release/release.json"
    target = tmp_path / "inputs/0/target_releases/target/release.json"

    def replay(*args: object) -> dict:
        if mutation == "base_content":
            base.write_text('{"changed":true}', encoding="utf-8")
        elif mutation == "target_membership":
            target.with_name("added.json").write_text("{}", encoding="utf-8")
        else:
            target.chmod(0o600)
        return copy.deepcopy(by_family[args[-1][-1]])

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    with pytest.raises(decision.NativeDecisionError, match="changed"):
        decision.build_native_file_decision(manifest, policy, *binaries)


@pytest.mark.parametrize("mutation", ["missing", "symlink", "fifo"])
def test_native_quality_refuses_invalid_release_tree_before_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    manifest, policy, binaries, _families = _case(tmp_path)
    root = tmp_path / "inputs/0/base_release"
    if mutation == "missing":
        shutil.rmtree(root)
    elif mutation == "symlink":
        (root / "alias.json").symlink_to(root / "release.json")
    else:
        os.mkfifo(root / "fifo")
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *_: pytest.fail("invalid tree replayed")
    )
    with pytest.raises(ValueError):
        decision.build_native_file_decision(manifest, policy, *binaries)


def test_native_quality_obeys_larger_frozen_sample_requirement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manifest, policy_path, binaries, families = _case(tmp_path)
    policy = json.loads(policy_path.read_text(encoding="utf-8"))
    policy["minimum_counts"]["positive"] = 200
    policy_path.write_text(json.dumps(policy), encoding="utf-8")
    for item in families:
        item["capture"]["runtime_binding"]["decision_policy_sha256"] = _sha(policy_path)
        item["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(item["capture"]))
    by_family = {item["suite"]["suite_id"]: item for item in families}
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *args: copy.deepcopy(by_family[args[-1][-1]])
    )
    result = decision.build_native_file_decision(manifest, policy_path, *binaries)
    assert result["status"] == "quality_reject"
    assert result["counts"]["positive"] == 120
    assert "positive_denominator_below_frozen_minimum" in result["reasons"]


@pytest.mark.parametrize("invalid", [False, 99, 30.0, "200"])
def test_native_quality_refuses_invalid_frozen_count(tmp_path: Path, invalid: object) -> None:
    manifest, policy_path, binaries, _families = _case(tmp_path)
    policy = json.loads(policy_path.read_text(encoding="utf-8"))
    policy["minimum_counts"]["positive"] = invalid
    policy_path.write_text(json.dumps(policy), encoding="utf-8")
    with pytest.raises(decision.NativeDecisionError, match="minimum"):
        decision.build_native_file_decision(manifest, policy_path, *binaries)


@pytest.mark.parametrize("index", [0, 1, 2])
def test_native_quality_refuses_fifo_binary_or_config_before_replay(tmp_path, monkeypatch, index):
    manifest, policy, binaries, _families = _case(tmp_path)
    path = binaries[index]
    path.unlink()
    os.mkfifo(path)
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *_: pytest.fail("FIFO reached replay")
    )
    with pytest.raises(ValueError, match="regular"):
        decision.build_native_file_decision(manifest, policy, *binaries)
