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
from tools.deployment import launcher


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


@pytest.fixture
def synthetic_replay_sources(monkeypatch: pytest.MonkeyPatch) -> None:
    """Quality-only fixtures replace the evaluator and have no Git checkouts."""
    monkeypatch.setattr(ev, "verify_repo", lambda repo, _commit: repo.resolve())


def _selected_native(tmp_path: Path) -> tuple[Path, str, launcher.Selection]:
    binary = tmp_path / "selected-searchd"
    binary.write_bytes(b"native daemon image")
    binary.chmod(0o700)
    root = tmp_path / "selected-state"
    root.mkdir()
    config = tmp_path / "selected-config.json"
    config.write_text(
        json.dumps({"state_root": str(root), "socket_path": str(tmp_path / "native.sock")})
    )
    config.chmod(0o600)
    receipt = launcher.NativeBootReceipt(
        "11" * 32,
        json.dumps(
            {
                "workspace": {
                    "tenant": [1] * 16,
                    "collection": [2] * 16,
                    "worktree": [3] * 16,
                    "slot": [4] * 16,
                },
                "epoch": 1,
                "producer": [5] * 16,
                "stream": [6] * 16,
                "stream_epoch": 1,
                "sequence": 1,
            },
            sort_keys=True,
            separators=(",", ":"),
        ),
        "22" * 32,
        "33" * 32,
        "44" * 32,
    )
    selected = launcher.proposed_native_selection(binary, root, config, {}, receipt)
    path = tmp_path / "selected.json"
    path.write_text(json.dumps(selected.to_wire(), sort_keys=True, separators=(",", ":")) + "\n")
    path.chmod(0o600)
    return path, _sha(path), selected


def test_selected_snapshot_uses_existing_launcher_wire_and_pinned_bytes(tmp_path: Path) -> None:
    path, digest, expected = _selected_native(tmp_path)
    raw, selected = decision.load_selected_snapshot(path, digest)
    assert selected == expected
    assert raw.sha256 == f"sha256:{digest}"
    path.write_text(path.read_text() + " ")
    with pytest.raises(ValueError, match="selection.*(digest|changed)"):
        decision.load_selected_snapshot(path, digest)
    path.write_bytes(b"x" * (launcher.MAX_SELECTION_BYTES + 1))
    with pytest.raises(ValueError, match="control document exceeds 65536-byte limit"):
        decision.load_selected_snapshot(path, digest)


def test_selected_runtime_requires_exact_boot_identity_and_root(tmp_path: Path) -> None:
    path, digest, selected = _selected_native(tmp_path)
    _raw, selected = decision.load_selected_snapshot(path, digest)
    binding = {
        "schema_version": 1,
        "kind": "linux_connected_peer_image_v1",
        "daemon_binary_sha256": selected.binary_sha256,
        "daemon_config_sha256": selected.native_config_sha256,
        "runner_binary_sha256": "55" * 32,
        "decision_policy_sha256": "66" * 32,
        "socket_path": str(tmp_path / "native.sock"),
        "config_path": str(selected.native_config),
        "command": "serve-native",
        "pid": 42,
        "uid": 501,
        "gid": 20,
        "peer_image_verified": True,
        "boot_identity": {
            "root_nonce_hex": selected.native_boot_receipt.root_nonce_hex,
            "manifest_operation": json.loads(selected.native_boot_receipt.manifest_operation_json),
            "manifest_object_hex": selected.native_boot_receipt.manifest_object_hex,
            "source_cut_digest_hex": selected.native_boot_receipt.source_cut_digest_hex,
        },
    }
    decision.check_selected_runtime(selected, binding, daemon_path=selected.binary)
    with pytest.raises(ValueError, match="selection binary path"):
        decision.check_selected_runtime(
            selected, binding, daemon_path=tmp_path / "different-same-bytes"
        )
    for field, value in (
        ("root_nonce_hex", "77" * 32),
        ("manifest_object_hex", "88" * 32),
        ("source_cut_digest_hex", "99" * 32),
        ("manifest_operation", {"wrong": "operation"}),
        (
            "manifest_operation",
            {**binding["boot_identity"]["manifest_operation"], "epoch": True},
        ),
    ):
        broken = copy.deepcopy(binding)
        broken["boot_identity"][field] = value
        with pytest.raises(ValueError, match="selection.*identity"):
            decision.check_selected_runtime(selected, broken)
    broken = copy.deepcopy(binding)
    broken["command"] = "serve-native-candidate"
    with pytest.raises(ValueError, match="selected.*command"):
        decision.check_selected_runtime(selected, broken)
    for field, value in (
        ("daemon_binary_sha256", "aa" * 32),
        ("daemon_config_sha256", "bb" * 32),
        ("config_path", str(tmp_path / "old-config.json")),
    ):
        broken = copy.deepcopy(binding)
        broken[field] = value
        with pytest.raises(ValueError, match="selection binary or config"):
            decision.check_selected_runtime(selected, broken)


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
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, synthetic_replay_sources: None
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *args, **_: copy.deepcopy(by_family[args[-1][-1]])
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


def test_native_quality_reuses_release_replay_only_within_one_decision(
    tmp_path, monkeypatch, synthetic_replay_sources
):
    from tools.benchmark.retrieval import composite_source_binding

    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    base = tmp_path / "inputs/0/base_release"
    target = tmp_path / "inputs/0/target_releases/target"
    replayed = []

    def verify_release(path):
        replayed.append(path)
        return ev.read_json(path / "release.json")

    def replay(*args, release_validator=None, **_expected):
        assert release_validator is not None
        release_validator(base)
        release_validator(target)
        return copy.deepcopy(by_family[args[-1][-1]])

    def verify_controlled(*_args, release_validator=None, scored_inputs):
        assert release_validator is not None
        assert scored_inputs.keys() == by_family.keys()
        for index, entry in enumerate(json.loads(manifest.read_bytes())["families"]):
            capture_path = tmp_path / f"inputs/{index}/capture.json"
            pinned = scored_inputs[entry["family_id"]]
            assert pinned.capture == (f"sha256:{_sha(capture_path)}", capture_path.stat().st_size)
            assert pinned.digests["cohort"] == f"sha256:{_sha(manifest)}"
            assert pinned.digests["decision_policy"] == f"sha256:{_sha(policy)}"
        release_validator(base)
        release_validator(target)
        return {
            family: item["report"]["capture_commitment_sha256"]
            for family, item in by_family.items()
        }

    monkeypatch.setattr(composite_source_binding._corpus_release(), "validate", verify_release)
    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    monkeypatch.setattr(decision, "verify_controlled_capture_runs", verify_controlled)
    for count in (1, 2):
        result = decision.build_native_file_decision(
            manifest,
            policy,
            *binaries,
            execution_evidence_root=tmp_path,
            execution_run_ids=["fixture"],
        )
        assert result["status"] == "quality_admit"
        assert result["controlled_capture_verified"] is True
        assert replayed == [base, target] * count


def test_native_quality_resolves_manifest_location_before_relative_release_inputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, synthetic_replay_sources: None
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    alias = tmp_path.parent / (tmp_path.name + "-alias")
    alias.symlink_to(tmp_path, target_is_directory=True)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *args, **_: copy.deepcopy(by_family[args[-1][-1]])
    )
    result = decision.build_native_file_decision(alias / manifest.name, policy, *binaries)
    assert result["status"] == "quality_admit"
    assert result["execution_binding_verified"] is False


@pytest.mark.parametrize("phase", ["family", "source_recheck"])
def test_native_quality_rejects_cohort_manifest_retargeted_during_replay(
    tmp_path,
    monkeypatch,
    synthetic_replay_sources,
    phase,
):
    manifest, policy, binaries, families = _case(tmp_path)
    alias = tmp_path / "cohort-alias.json"
    alias.symlink_to(manifest)
    replacement = tmp_path / "replacement-cohort.json"
    replacement.write_bytes(manifest.read_bytes())
    by_family = {item["suite"]["suite_id"]: item for item in families}

    def retarget():
        if alias.readlink() == manifest:
            alias.unlink()
            alias.symlink_to(replacement)

    def replay(*args, **_options):
        if phase == "family":
            retarget()
        return copy.deepcopy(by_family[args[-1][-1]])

    if phase == "source_recheck":
        verify_repo = ev.verify_repo

        def verify_and_retarget(repo, commit):
            result = verify_repo(repo, commit)
            retarget()
            return result

        monkeypatch.setattr(ev, "verify_repo", verify_and_retarget)
    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    with pytest.raises(decision.NativeDecisionError, match="inputs changed during replay"):
        decision.build_native_file_decision(alias, policy, *binaries)
    assert alias.readlink() == replacement


def test_native_quality_keeps_legacy_config_alias_pinned_during_replay(
    tmp_path,
    monkeypatch,
    synthetic_replay_sources,
):
    manifest, policy, binaries, families = _case(tmp_path)
    daemon, config, runner = binaries
    alias = tmp_path / "config-alias.json"
    alias.symlink_to(config)
    replacement = tmp_path / "replacement-config.json"
    replacement.write_bytes(config.read_bytes())
    by_family = {item["suite"]["suite_id"]: item for item in families}

    def replay(*args, **_options):
        if alias.readlink() == config:
            alias.unlink()
            alias.symlink_to(replacement)
        return copy.deepcopy(by_family[args[-1][-1]])

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    with pytest.raises(decision.NativeDecisionError, match="inputs changed during replay"):
        decision.build_native_file_decision(manifest, policy, daemon, alias, runner)
    assert alias.readlink() == replacement


def test_native_quality_keeps_each_family_capture_path_pinned(
    tmp_path,
    monkeypatch,
    synthetic_replay_sources,
):
    manifest, policy, binaries, families = _case(tmp_path)
    entries = json.loads(manifest.read_bytes())["families"]
    alias = tmp_path / entries[1]["capture"]
    original = tmp_path / "family-1-capture.json"
    alias.rename(original)
    alias.symlink_to(original)
    other_family = tmp_path / entries[2]["capture"]
    by_family = {item["suite"]["suite_id"]: item for item in families}
    observed = {}

    def replay(*args, **_expected):
        family_id = args[-1][-1]
        observed[family_id] = args[3]
        if family_id == entries[0]["family_id"]:
            alias.unlink()
            alias.symlink_to(other_family)
        return copy.deepcopy(by_family[family_id])

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    with pytest.raises(decision.NativeDecisionError, match="inputs changed during replay"):
        decision.build_native_file_decision(manifest, policy, *binaries)
    assert observed[entries[1]["family_id"]] == original


@pytest.mark.parametrize(
    "mutation",
    [
        "stale_daemon",
        "stale_policy",
        "missing_runtime",
        "false_peer",
        "malformed_runtime_command",
        "mixed_serving_modes",
        "bad_recall",
        "bad_identifier",
        "bad_negative",
        "short_denominator",
        "slow_query",
    ],
)
def test_native_quality_refuses_or_rejects_broken_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
    synthetic_replay_sources: None,
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
    elif mutation == "malformed_runtime_command":
        families[0]["capture"]["runtime_binding"]["command"] = ["serve-native-candidate"]
    elif mutation == "mixed_serving_modes":
        families[0]["capture"]["runtime_binding"]["command"] = "serve-native"
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
        ev, "load_native_file_replay", lambda *args, **_: copy.deepcopy(by_family[args[-1][-1]])
    )
    if mutation == "mixed_serving_modes":
        with pytest.raises(decision.NativeDecisionError, match="families mix serving modes"):
            decision.build_native_file_decision(manifest, policy, *binaries)
    elif mutation in {
        "stale_daemon",
        "stale_policy",
        "missing_runtime",
        "false_peer",
        "malformed_runtime_command",
    }:
        with pytest.raises((ev.EvidenceError, decision.NativeDecisionError)):
            decision.build_native_file_decision(manifest, policy, *binaries)
    else:
        result = decision.build_native_file_decision(manifest, policy, *binaries)
        assert result["status"] == "quality_reject"
        assert result["product_default_decision"] is False
        if mutation == "short_denominator":
            assert "positive_denominator_below_frozen_minimum" in result["reasons"]


def test_controlled_capture_refuses_nonstring_run_id(tmp_path: Path) -> None:
    manifest, policy, (daemon, config, runner), families = _case(tmp_path)
    run_ids = [[], *(f"capture-{index}" for index in range(1, len(families)))]

    with pytest.raises(decision.NativeDecisionError, match="controlled run inventory"):
        decision.verify_controlled_capture_runs(
            manifest,
            policy,
            daemon,
            config,
            runner,
            tmp_path,
            run_ids,
        )


def test_native_quality_refuses_input_changed_during_replay(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    synthetic_replay_sources,
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    changed = tmp_path / "inputs/0/capture.json"

    def replay(*args: object, **_kwargs: object) -> dict:
        changed.write_text('{"changed":true}', encoding="utf-8")
        return copy.deepcopy(by_family[args[-1][-1]])

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    with pytest.raises(decision.NativeDecisionError, match="changed"):
        decision.build_native_file_decision(manifest, policy, *binaries)


@pytest.mark.parametrize("replacement", [b'\x7b"transient":true}', b"[]"])
def test_native_quality_binds_bytes_consumed_during_capture_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, replacement: bytes
) -> None:
    manifest, policy, binaries, _families = _case(tmp_path)
    capture = tmp_path / "inputs/0/capture.json"
    original = capture.read_bytes()
    reader = ev._load_native_file_inputs

    def replay(*args, expected_capture, **_options):
        if args[-1][-1] == "family-0":
            capture.write_bytes(replacement)
            try:
                reader(*args[:4], expected_capture=expected_capture)
            finally:
                capture.write_bytes(original)
        pytest.fail("unbound capture reached scorer")

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    with pytest.raises(ValueError, match="capture bytes differ"):
        decision.build_native_file_decision(manifest, policy, *binaries)
    assert capture.read_bytes() == original


@pytest.mark.parametrize("role", ["capture", "suite"])
def test_native_replay_rejects_changed_length_before_reading_payload(tmp_path, monkeypatch, role):
    from tools.benchmark import evidence

    original = b"{}"
    expected = f"sha256:{hashlib.sha256(original).hexdigest()}", len(original)
    suite, pack, capture = (tmp_path / name for name in ("suite", "pack", "capture"))
    for path in (suite, pack, capture):
        path.write_bytes(original)
    changed = capture if role == "capture" else suite
    changed.write_bytes(original + b" ")
    consume_regular = evidence._consume_regular_file

    class NoPayloadRead:
        def __init__(self, handle):
            self.fileno = handle.fileno

        def read(self, *_args):
            pytest.fail("size mismatch reached payload read")

    def consume_without_read(path, consume):
        if path == changed:
            return consume_regular(path, lambda handle: consume(NoPayloadRead(handle)))
        return consume_regular(path, consume)

    monkeypatch.setattr(evidence, "_consume_regular_file", consume_without_read)
    with pytest.raises(ValueError, match="bytes differ.*commitment"):
        ev._load_native_file_inputs(
            tmp_path, suite, pack, capture, **{f"expected_{role}": expected}
        )


@pytest.mark.parametrize("changed", ["source_universe", "source_binding"])
def test_native_source_reader_binds_bytes_consumed_during_replay(
    tmp_path: Path, changed: str
) -> None:
    from tools.benchmark.retrieval import composite_source_binding

    universe = tmp_path / "universe.json"
    binding = tmp_path / "binding.json"
    universe.write_bytes(b"{}")
    binding.write_bytes(b"{}")
    selected = universe if changed == "source_universe" else binding
    original = selected.read_bytes()
    expected = ("sha256:" + hashlib.sha256(original).hexdigest(), len(original))
    selected.write_bytes(b'{"transient":true}')
    try:
        with pytest.raises(ValueError, match="bytes differ from frozen commitment"):
            composite_source_binding.verify(
                None,
                None,
                None,
                None,
                universe,
                binding,
                tmp_path,
                tmp_path,
                "family-0",
                **{f"expected_{changed}": expected},
            )
    finally:
        selected.write_bytes(original)
    assert selected.read_bytes() == original


def test_native_reviewed_labels_reader_binds_bytes_consumed_during_replay(
    tmp_path: Path,
) -> None:
    receipt = tmp_path / "reviewed.json"
    original = b"{}"
    receipt.write_bytes(original)
    receipt.write_bytes(b'{"transient":true}')
    try:
        with pytest.raises(ValueError, match="reviewed labels bytes differ"):
            ev.load_native_file_replay(
                tmp_path,
                tmp_path / "suite.json",
                tmp_path / "pack.json",
                tmp_path / "capture.json",
                reviewed_labels=receipt,
                expected_reviewed_labels=(
                    "sha256:" + hashlib.sha256(original).hexdigest(),
                    len(original),
                ),
            )
    finally:
        receipt.write_bytes(original)
    assert receipt.read_bytes() == original


@pytest.mark.parametrize("change_on_disk", [False, True])
def test_controlled_capture_must_match_scored_bytes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    synthetic_replay_sources,
    change_on_disk: bool,
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *args, **_: copy.deepcopy(by_family[args[-1][-1]])
    )
    capture_paths = {
        f"family-{i}": tmp_path / f"inputs/{i}/capture.json" for i in range(len(families))
    }
    scored = {
        family: item["report"]["capture_commitment_sha256"] for family, item in by_family.items()
    }

    def verify(*_args: object, **_kwargs: object) -> dict[str, str]:
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
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    synthetic_replay_sources,
    mutation: str,
) -> None:
    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    base = tmp_path / "inputs/0/base_release/release.json"
    target = tmp_path / "inputs/0/target_releases/target/release.json"

    def replay(*args: object, **_kwargs: object) -> dict:
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


def test_native_quality_rechecks_git_head_after_aggregation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools.ci.tests.test_native_file_cohort import (
        _advance_git_head,
        _bind_fixture_to_git,
        _git_checkout,
    )

    manifest, policy, binaries, families = _case(tmp_path)
    cohort = json.loads(manifest.read_bytes())
    by_family = {}
    for index, (entry, item) in enumerate(zip(cohort["families"], families, strict=True)):
        entry["repo"] = f"git-family-{index}"
        repo = tmp_path / entry["repo"]
        _git_checkout(repo)
        by_family[entry["family_id"]] = _bind_fixture_to_git(item, repo)
    manifest.write_bytes(ev.canonical(cohort))
    frozen_policy = json.loads(policy.read_bytes())
    frozen_policy["cohort_manifest_sha256"] = _sha(manifest)
    policy.write_bytes(ev.canonical(frozen_policy))
    for item in by_family.values():
        item["capture"]["runtime_binding"]["decision_policy_sha256"] = _sha(policy)
        item["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(item["capture"]))
    monkeypatch.setattr(
        ev,
        "load_native_file_replay",
        lambda *args, **_options: copy.deepcopy(by_family[args[-1][-1]]),
    )
    original_aggregate = decision.native_file_evidence.repository_cluster_ci_from_native_families
    changed_repo = tmp_path / cohort["families"][0]["repo"]

    def aggregate(*args):
        result = original_aggregate(*args)
        _advance_git_head(changed_repo)
        return result

    monkeypatch.setattr(
        decision.native_file_evidence,
        "repository_cluster_ci_from_native_families",
        aggregate,
    )
    with pytest.raises(ev.EvidenceError, match="HEAD|checkout"):
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
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, synthetic_replay_sources: None
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
        ev, "load_native_file_replay", lambda *args, **_: copy.deepcopy(by_family[args[-1][-1]])
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


@pytest.mark.parametrize("changed_input", ["receipt_bytes", "receipt_path", "review_manifest_path"])
def test_native_quality_replays_exact_review_roster_and_checks_receipt_freshness(
    tmp_path,
    monkeypatch,
    synthetic_replay_sources,
    changed_input,
):
    manifest, policy, binaries, families = _case(tmp_path)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    reviewed = tmp_path / "reviews.json"
    rows = {family: f"review-{index}.json" for index, family in enumerate(by_family)}
    reviewed.write_text(json.dumps(rows), encoding="utf-8")
    for name in rows.values():
        (tmp_path / name).write_bytes(b"{}")
    first_receipt = tmp_path / next(iter(rows.values()))
    declared = {
        "receipt_path": first_receipt,
        "review_manifest_path": reviewed,
    }.get(changed_input)
    pinned_paths = {family: tmp_path / receipt for family, receipt in rows.items()}
    if declared is not None:
        pinned = declared.with_suffix(".original.json")
        declared.rename(pinned)
        declared.symlink_to(pinned)
        replacement = declared.with_suffix(".replacement.json")
        replacement.write_bytes(pinned.read_bytes())
        if changed_input == "receipt_path":
            pinned_paths[next(iter(by_family))] = pinned
    original_manifest = manifest.read_bytes()
    observed = {}

    def replay(*args, reviewed_labels, **_options):
        family = args[-1][-1]
        observed[family] = reviewed_labels
        return copy.deepcopy(by_family[family])

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    result = decision.build_native_file_decision(
        manifest, policy, *binaries, reviewed_labels_path=reviewed
    )
    assert result["reviewed_labels_manifest_sha256"] == _sha(reviewed)
    assert observed == pinned_paths
    assert manifest.read_bytes() == original_manifest

    def changed(*args, reviewed_labels, **options):
        if declared is None:
            reviewed_labels.write_bytes(b"{ } ")
        elif declared.readlink() != replacement:
            declared.unlink()
            declared.symlink_to(replacement)
        return replay(*args, reviewed_labels=reviewed_labels, **options)

    monkeypatch.setattr(ev, "load_native_file_replay", changed)
    with pytest.raises(ValueError, match="inputs changed during replay"):
        decision.build_native_file_decision(
            manifest, policy, *binaries, reviewed_labels_path=reviewed
        )


@pytest.mark.parametrize("via_alias", [False, True])
def test_native_quality_rejects_review_receipt_reusing_a_frozen_policy_path(
    tmp_path,
    monkeypatch,
    synthetic_replay_sources,
    via_alias,
):
    manifest, policy, binaries, families = _case(tmp_path)
    declared_policy = policy
    if via_alias:
        declared_policy = tmp_path / "policy-alias.json"
        declared_policy.symlink_to(policy)
    by_family = {item["suite"]["suite_id"]: item for item in families}
    reviewed = tmp_path / "reviews.json"
    rows = {family: f"review-{index}.json" for index, family in enumerate(by_family)}
    rows[next(iter(rows))] = policy.name
    reviewed.write_text(json.dumps(rows), encoding="utf-8")
    for receipt in rows.values():
        if receipt != policy.name:
            (tmp_path / receipt).write_bytes(b"{}")
    verify_policy = decision._policy

    def verify_and_replace_policy(*args):
        result = verify_policy(*args)
        policy.write_bytes(b"{}")
        return result

    monkeypatch.setattr(decision, "_policy", verify_and_replace_policy)
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *args, **_: copy.deepcopy(by_family[args[-1][-1]])
    )
    with pytest.raises(
        decision.NativeDecisionError, match="input path is reused with different bytes"
    ):
        decision.build_native_file_decision(
            manifest, declared_policy, *binaries, reviewed_labels_path=reviewed
        )


def test_native_quality_binds_independent_family_configs_to_one_recipe(
    tmp_path, monkeypatch, synthetic_replay_sources
):
    manifest, policy, binaries, families = _case(tmp_path)
    cohort = json.loads(manifest.read_bytes())
    cohort["schema_version"] = 2
    config_paths = []
    for index, (entry, replay) in enumerate(zip(cohort["families"], families, strict=True)):
        path = tmp_path / f"daemon-{index}.json"
        socket = str(tmp_path / f"native-{index}.sock")
        path.write_bytes(
            ev.canonical(
                {
                    "socket_path": socket,
                    "state_root": str(tmp_path / f"state-{index}"),
                    "work_limits": {"reads": 1_000_000},
                }
            )
        )
        config_paths.append(path)
        entry.update(daemon_config=path.name, daemon_config_sha256=_sha(path))
        replay["capture"]["runtime_binding"].update(
            config_path=str(path),
            socket_path=socket,
            daemon_config_sha256=_sha(path),
        )
    manifest.write_bytes(ev.canonical(cohort))
    policy_value = json.loads(policy.read_bytes())
    policy_value["cohort_manifest_sha256"] = _sha(manifest)
    policy.write_bytes(ev.canonical(policy_value))
    for replay in families:
        replay["capture"]["runtime_binding"]["decision_policy_sha256"] = _sha(policy)
        replay["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(replay["capture"]))
    by_family = {item["suite"]["suite_id"]: item for item in families}
    monkeypatch.setattr(ev, "load_native_file_replay", lambda *args, **_: by_family[args[-1][-1]])
    daemon, _, runner = binaries
    result = decision.build_native_file_decision(manifest, policy, daemon, None, runner)
    assert result["status"] == "quality_admit"

    for family in families:
        family["capture"]["runtime_binding"]["command"] = "serve-native"
        family["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(family["capture"]))
    with pytest.raises(ValueError, match="requires a pinned selection"):
        decision.build_native_file_decision(manifest, policy, daemon, None, runner)
    for family in families:
        family["capture"]["runtime_binding"]["command"] = "serve-native-candidate"
        family["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(family["capture"]))

    families[0]["capture"]["runtime_binding"]["command"] = "serve-native"
    with pytest.raises(ValueError, match="requires a pinned selection"):
        decision.build_native_file_decision(manifest, policy, daemon, None, runner)
    families[0]["capture"]["runtime_binding"]["command"] = "serve-native-candidate"

    families[0]["capture"]["runtime_binding"] = families[1]["capture"]["runtime_binding"]
    with pytest.raises(ValueError, match="connected daemon, config, runner and policy"):
        decision.build_native_file_decision(manifest, policy, daemon, None, runner)

    config_paths[-1].write_bytes(config_paths[-1].read_bytes() + b"\n")
    with pytest.raises(ValueError, match="family daemon config digest differs"):
        decision.build_native_file_decision(manifest, policy, daemon, None, runner)

    changed = json.loads(config_paths[-1].read_bytes())
    changed["work_limits"]["reads"] += 1
    config_paths[-1].write_bytes(ev.canonical(changed))
    cohort["families"][-1]["daemon_config_sha256"] = _sha(config_paths[-1])
    manifest.write_bytes(ev.canonical(cohort))
    policy_value["cohort_manifest_sha256"] = _sha(manifest)
    policy.write_bytes(ev.canonical(policy_value))
    with pytest.raises(
        ValueError, match="family daemon configs differ beyond state root and socket"
    ):
        decision.build_native_file_decision(manifest, policy, daemon, None, runner)


@pytest.mark.parametrize("role", ["selection_path", "daemon_config"])
def test_native_quality_refuses_same_bytes_runtime_path_replacement_during_replay(
    tmp_path,
    monkeypatch,
    role,
    synthetic_replay_sources,
):
    manifest, policy, binaries, families = _case(tmp_path)
    _, _, selected = _selected_native(tmp_path)
    cohort = json.loads(manifest.read_bytes())
    for index, (entry, replay) in enumerate(zip(cohort["families"], families, strict=True)):
        state_root = tmp_path / f"selected-state-{index}"
        state_root.mkdir()
        config = tmp_path / f"selected-config-{index}.json"
        socket = str(tmp_path / f"selected-{index}.sock")
        config.write_bytes(ev.canonical({"state_root": str(state_root), "socket_path": socket}))
        config.chmod(0o600)
        selection = tmp_path / f"selected-{index}.json"
        current = launcher.proposed_native_selection(
            selected.binary,
            state_root,
            config,
            {},
            selected.native_boot_receipt,
        )
        selection.write_bytes(ev.canonical(current.to_wire()))
        selection.chmod(0o600)
        declared_config = config
        if index == 0 and role == "daemon_config":
            declared_config = config.with_suffix(".alias.json")
            declared_config.symlink_to(config)
        entry.update(
            daemon_config=declared_config.name,
            daemon_config_sha256=_sha(config),
            selection_path=selection.name,
            selection_sha256=_sha(selection),
        )
        receipt = current.native_boot_receipt
        replay["capture"]["runtime_binding"].update(
            daemon_config_sha256=_sha(config),
            socket_path=socket,
            config_path=str(config),
            command="serve-native",
            boot_identity={
                "root_nonce_hex": receipt.root_nonce_hex,
                "manifest_operation": json.loads(receipt.manifest_operation_json),
                "manifest_object_hex": receipt.manifest_object_hex,
                "source_cut_digest_hex": receipt.source_cut_digest_hex,
            },
        )
    cohort["schema_version"] = 2
    manifest.write_bytes(ev.canonical(cohort))
    frozen_policy = json.loads(policy.read_bytes())
    frozen_policy["cohort_manifest_sha256"] = _sha(manifest)
    policy.write_bytes(ev.canonical(frozen_policy))
    for replay in families:
        replay["capture"]["runtime_binding"]["decision_policy_sha256"] = _sha(policy)
        replay["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(replay["capture"]))
    by_family = {item["suite"]["suite_id"]: item for item in families}
    _, _, runner = binaries
    monkeypatch.setattr(ev, "load_native_file_replay", lambda *args, **_: by_family[args[-1][-1]])
    result = decision.build_native_file_decision(manifest, policy, selected.binary, None, runner)
    assert result["status"] == "quality_admit"

    original = tmp_path / cohort["families"][0][role]
    replacement = tmp_path / "same-runtime-input-bytes.json"
    replacement.write_bytes(original.read_bytes())
    replacement.chmod(0o600)
    replaced = False

    def replay(*args, **_options):
        nonlocal replaced
        if not replaced:
            original.unlink()
            original.symlink_to(replacement)
            replaced = True
        return by_family[args[-1][-1]]

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    error = "selection" if role == "selection_path" else "inputs changed during replay"
    with pytest.raises(decision.NativeDecisionError, match=error):
        decision.build_native_file_decision(manifest, policy, selected.binary, None, runner)
    assert replaced


def test_signed_native_quality_requires_controlled_v2_before_scoring(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    manifest, policy, binaries, _ = _case(tmp_path)
    monkeypatch.setattr(
        ev,
        "load_native_file_replay",
        lambda *_, **__: pytest.fail("unsigned family scored"),
    )
    with pytest.raises(decision.NativeDecisionError, match="controlled execution root and run IDs"):
        decision.build_native_file_decision(
            manifest,
            policy,
            *binaries,
            require_signed_install=True,
        )
    with pytest.raises(decision.NativeDecisionError, match="v2 selected cohort"):
        decision.build_native_file_decision(
            manifest,
            policy,
            *binaries,
            require_signed_install=True,
            execution_evidence_root=tmp_path,
            execution_run_ids=["unused"],
        )


@pytest.mark.parametrize(
    "drift",
    [
        None,
        "generation",
        "other_selection",
        "missing_reviewed_labels",
        "standalone_receipt",
        "review_digest",
        "review_changed_after_replay",
    ],
)
def test_signed_native_quality_verifies_install_before_scoring_and_reports_exact_scope(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    synthetic_replay_sources,
    drift: str | None,
) -> None:
    import importlib

    from tools.benchmark.evidence import EvidenceError
    from tools.benchmark.retrieval import holdout_review

    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
    capture = importlib.import_module("native_file_capture")
    manifest, policy, binaries, families = _case(tmp_path)
    _selection_path, _, selected = _selected_native(tmp_path)
    cohort = json.loads(manifest.read_bytes())
    for index, (entry, replay) in enumerate(zip(cohort["families"], families, strict=True)):
        state_root = tmp_path / f"signed-state-{index}"
        state_root.mkdir()
        config = tmp_path / f"signed-config-{index}.json"
        socket = str(tmp_path / f"signed-{index}.sock")
        config.write_bytes(ev.canonical({"state_root": str(state_root), "socket_path": socket}))
        config.chmod(0o600)
        selection = tmp_path / f"signed-selection-{index}.json"
        current = launcher.proposed_native_selection(
            selected.binary,
            state_root,
            config,
            {},
            selected.native_boot_receipt,
        )
        selection.write_bytes(ev.canonical(current.to_wire()))
        selection.chmod(0o600)
        entry.update(
            daemon_config=config.name,
            daemon_config_sha256=_sha(config),
            selection_path=selection.name,
            selection_sha256=_sha(selection),
        )
        receipt = current.native_boot_receipt
        replay["capture"]["runtime_binding"].update(
            daemon_config_sha256=_sha(config),
            socket_path=socket,
            config_path=str(config),
            command="serve-native",
            boot_identity={
                "root_nonce_hex": receipt.root_nonce_hex,
                "manifest_operation": json.loads(receipt.manifest_operation_json),
                "manifest_object_hex": receipt.manifest_object_hex,
                "source_cut_digest_hex": receipt.source_cut_digest_hex,
            },
        )
    cohort["schema_version"] = 2
    manifest.write_bytes(ev.canonical(cohort))
    frozen_policy = json.loads(policy.read_bytes())
    frozen_policy["cohort_manifest_sha256"] = _sha(manifest)
    policy.write_bytes(ev.canonical(frozen_policy))
    for replay in families:
        replay["capture"]["runtime_binding"]["decision_policy_sha256"] = _sha(policy)
        replay["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(replay["capture"]))
    by_family = {item["suite"]["suite_id"]: item for item in families}
    reviewed = tmp_path / "signed-reviewed-labels.json"
    review_paths = {}
    for index, family in enumerate(by_family):
        path = tmp_path / f"signed-review-{index}.json"
        review_paths[family] = path
        inputs = {}
        for role in (
            "contexts",
            "control_pools",
            "review_1",
            "review_2",
            "adjudication",
            "task_contracts",
        ):
            source = tmp_path / f"signed-review-{index}-{role}.json"
            source.write_bytes(ev.canonical({"family": family, "role": role}))
            inputs[role] = {"path": source.name, "sha256": _sha(source)}
        if index == 0 and drift == "review_digest":
            inputs["review_1"]["sha256"] = "0" * 64
        path.write_bytes(
            ev.canonical(
                {"status": "adjudicated_file_labels_unqualified", "qualified": False}
                if index == 0 and drift == "standalone_receipt"
                else {
                    "schema_version": "native-file-review-source/v1",
                    "seed": index,
                    "inputs": inputs,
                }
            )
        )
    reviewed.write_bytes(ev.canonical({family: path.name for family, path in review_paths.items()}))
    verified = False
    identity = {
        "active_generation_sha256": "a" * 64,
        "release_manifest_sha256": "b" * 64,
        "q_elf_sha256": "c" * 64,
    }
    representative = cohort["families"][0]
    plan = {
        "selection_path": str(tmp_path / representative["selection_path"]),
        "selection_sha256": representative["selection_sha256"],
        capture.SIGNED_INSTALL: {"payload": identity},
    }
    drifted = False
    checked = []
    labels_replayed = []

    def verify(
        *_args,
        require_signed_install,
        signed_install_identity,
        signed_install_recheck,
        **_options,
    ):
        nonlocal verified
        assert require_signed_install is True
        signed_install_identity.update(identity)
        signed_install_recheck.append(
            (plan, Path(plan["selection_path"]), plan["selection_sha256"])
        )
        verified = True
        return {
            family: replay["report"]["capture_commitment_sha256"]
            for family, replay in by_family.items()
        }

    def replay_labels(_repo, _suite, _pack, capture_path, **options):
        assert verified, "signed installation must be verified before replaying reviews"
        family = options["source_inputs"][-1]
        assert options["expected_capture_sha256"] == _sha(capture_path)
        assert options["completed_forms"] == [
            {"family": family, "role": "review_1"},
            {"family": family, "role": "review_2"},
        ]
        assert options["adjudicated_form"] == {"family": family, "role": "adjudication"}
        labels_replayed.append(family)
        return {"family": family, "fixture": "replayed"}

    def replay(*args, reviewed_labels=None, **_options):
        nonlocal drifted
        assert verified, "signed installation must be verified before scoring"
        family = args[-1][-1]
        assert reviewed_labels == ev.canonical({"family": family, "fixture": "replayed"})
        if drift == "generation":
            drifted = True
        if drift == "review_changed_after_replay":
            original = tmp_path / f"{review_paths[family].stem}-review_1.json"
            original.write_bytes(rb'{"changed":true}')
        return copy.deepcopy(by_family[family])

    def check_install(actual_plan, actual_selected, policy_sha):
        assert actual_plan is plan
        assert actual_selected.state_root == tmp_path / "signed-state-0"
        assert policy_sha == _sha(policy)
        checked.append(True)
        if drifted:
            raise EvidenceError("Native signed installation differs: active generation changed")
        if drift == "other_selection":
            other_selection = tmp_path / cohort["families"][1]["selection_path"]
            other_selection.write_bytes(other_selection.read_bytes() + b" ")

    monkeypatch.setattr(decision, "verify_controlled_capture_runs", verify)
    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    monkeypatch.setattr(capture, "_verify_signed_install", check_install)
    monkeypatch.setattr(holdout_review, "replay_native_review_labels", replay_labels)
    options = {
        "execution_evidence_root": tmp_path,
        "execution_run_ids": ["fixture"],
        "require_signed_install": True,
    }
    if drift != "missing_reviewed_labels":
        options["reviewed_labels_path"] = reviewed
    entry_errors = {
        "missing_reviewed_labels": "independently reviewed labels",
        "standalone_receipt": "requires original review source inputs",
        "review_digest": "original review source differs from its manifest",
    }
    if drift in entry_errors:
        with pytest.raises(decision.NativeDecisionError, match=entry_errors[drift]):
            decision.build_native_file_decision(
                manifest,
                policy,
                selected.binary,
                None,
                binaries[2],
                **options,
            )
        assert not verified
        return
    expected_errors = {
        "generation": (EvidenceError, "active generation changed"),
        "other_selection": (decision.NativeDecisionError, "selection digest changed"),
        "review_changed_after_replay": (
            decision.NativeDecisionError,
            "inputs changed during replay",
        ),
    }
    if drift is not None:
        error, message = expected_errors[drift]
        with pytest.raises(error, match=message):
            decision.build_native_file_decision(
                manifest,
                policy,
                selected.binary,
                None,
                binaries[2],
                **options,
            )
        assert checked == [True]
        return
    result = decision.build_native_file_decision(
        manifest,
        policy,
        selected.binary,
        None,
        binaries[2],
        **options,
    )
    assert checked == [True]
    assert labels_replayed == list(by_family)
    assert result["status"] == "quality_admit"
    assert result["signed_install_verified"] is True
    assert result["signed_install"] == identity
    assert result["reviewed_labels_manifest_sha256"] == _sha(reviewed)
    assert result["review_source_manifest_sha256"] == {
        family: _sha(path) for family, path in review_paths.items()
    }
    assert result["product_default_decision"] is False
    assert result["runtime_qualified"] is False


@pytest.mark.parametrize(
    "changed_key",
    ["active_generation_sha256", "release_manifest_sha256", "q_elf_sha256"],
)
def test_signed_native_controlled_replay_refuses_mixed_installed_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    changed_key: str,
) -> None:
    import importlib

    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
    capture = importlib.import_module("native_file_capture")
    evidence = importlib.import_module("evidence")
    bridge = importlib.import_module("evidence_bridge")
    manifest, policy, daemon, runner = (
        tmp_path / name for name in ("cohort.json", "policy.json", "daemon", "runner")
    )
    config = tmp_path / "config.json"
    entries = [{"family_id": f"family-{index}"} for index in range(2)]
    monkeypatch.setattr(
        ev,
        "_native_cohort_manifest",
        lambda _path: (b"fixture", {"schema_version": 2, "families": entries}),
    )
    monkeypatch.setattr(bridge, "source_identity", lambda *_args: {"closure_digest": "fixture"})
    monkeypatch.setattr(evidence, "file_digest", lambda _path: ("sha256:lock", 4))
    monkeypatch.setattr(
        decision,
        "_cohort_runtime",
        lambda _manifest, family, _config: (
            config,
            "config-sha",
            (tmp_path / f"selection-{family['family_id']}", "selection-sha"),
        ),
    )

    class Store:
        def __init__(self, root):
            self.root = root

    monkeypatch.setattr(evidence, "RunStore", Store)
    monkeypatch.setattr(
        capture,
        "load_published_run",
        lambda _store, run_id, **_options: {
            "case_id": run_id,
            "source": {"closure_digest": "fixture"},
            "build": {"lockfile_digest": "sha256:lock"},
        },
    )
    monkeypatch.setattr(
        capture,
        "_archived_raw",
        lambda _store, record, name: record["case_id"],
    )
    identities = [
        {
            "active_generation_sha256": "a" * 64,
            "release_manifest_sha256": "b" * 64,
            "q_elf_sha256": "c" * 64,
        }
        for _ in entries
    ]
    identities[1][changed_key] = "d" * 64

    def plan(family_id, *, require_signed_install):
        assert require_signed_install is True
        index = int(family_id.rsplit("-", 1)[1])
        return {
            "family_id": family_id,
            "cohort_manifest": str(manifest),
            "decision_policy": str(policy),
            "daemon_binary": str(daemon),
            "daemon_config": str(config),
            "runner_binary": str(runner),
            "selection_path": str(tmp_path / f"selection-{family_id}"),
            "selection_sha256": "selection-sha",
            capture.SIGNED_INSTALL: {"payload": identities[index]},
        }

    monkeypatch.setattr(capture, "_plan", plan)
    replayed = []

    def replay(_store, record, *, release_validator, require_signed_install):
        assert require_signed_install is True
        replayed.append(record["case_id"])
        return record["case_id"]

    monkeypatch.setattr(capture, "replay_run", replay)
    with pytest.raises(decision.NativeDecisionError, match="differs across cohort families"):
        decision.verify_controlled_capture_runs(
            manifest,
            policy,
            daemon,
            config,
            runner,
            tmp_path,
            [entry["family_id"] for entry in entries],
            require_signed_install=True,
        )
    assert replayed == ["family-0", "family-1"]
