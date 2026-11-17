"""Native capture orchestration tests; fake runner is not an SDK/product proof."""

import copy
import hashlib
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import native_file_capture as capture
from evidence import RunStore, sample_evidence
from registry import load_registry

from tools.benchmark.retrieval import evaluator, native_file_decision
from tools.ci.tests.test_native_file_decision import _case


def _registered_owner() -> dict:
    registry = copy.deepcopy(load_registry())
    registry["producers"][capture.FAMILY] = {
        "kind": "python-module",
        "module": "tools/benchmark/native_file_capture.py",
        "argv": [],
        "outputs": [],
    }
    registry["validators"][capture.FAMILY] = {
        "kind": "python-module",
        "module": "tools/benchmark/native_file_capture.py",
        "argv": ["validate"],
    }
    registry["families"][capture.FAMILY] = {
        "title": "Controlled Native file SDK capture",
        "purpose": "retrieval",
        "payload": "proof",
        "result_unit": "count",
        "producer": capture.FAMILY,
        "validator": capture.FAMILY,
        "scorer": "none",
        "closure": "retrieval",
        "host_policy": "any",
        "gate_tier": "contract",
        "sample_floor": 0,
        "baseline": "none",
        "native_schema": "native-file-sdk-capture:v1",
        "authority": "registry",
    }
    registry["profiles"][capture.PROFILE] = {
        "description": "Controlled Native file SDK capture",
        "families": [capture.FAMILY],
    }
    return registry


def test_native_capture_registration_refuses_other_producer():
    registry = _registered_owner()
    capture.require_registered_owner(registry)
    registry["producers"][capture.FAMILY]["kind"] = "recorded-input"
    with pytest.raises(ValueError, match="registration"):
        capture.require_registered_owner(registry)


def test_current_native_capture_registration_is_wired():
    capture.require_registered_owner(load_registry())


@pytest.mark.parametrize(
    ("section", "field", "value"),
    [
        ("families", "purpose", "recorded-experiment"),
        ("families", "sample_floor", 1),
        ("producers", "argv", ["--unexpected"]),
        ("producers", "outputs", ["forged.json"]),
        ("validators", "argv", ["other"]),
    ],
)
def test_native_capture_registration_rejects_drift(section, field, value):
    registry = _registered_owner()
    registry[section][capture.FAMILY][field] = value
    with pytest.raises(ValueError, match="registration"):
        capture.require_registered_owner(registry)


def test_owned_capture_invokes_real_subprocess_and_binds_output(tmp_path):
    runner = tmp_path / "runner.py"
    runner.write_text(
        "#!/usr/bin/env python3\n"
        "import pathlib, sys\n"
        "pathlib.Path(sys.argv[sys.argv.index('--out') + 1]).write_text('{\"profile\":\"native_file\"}')\n"
    )
    runner.chmod(0o755)
    output = tmp_path / "capture.json"
    result = capture.execute_owned_native(
        runner,
        ["--query-pack", str(tmp_path / "pack.json"), "--out", str(output)],
        cwd=tmp_path,
        log_dir=tmp_path / "logs",
        timeout=5,
    )
    assert json.loads(output.read_text()) == {"profile": "native_file"}
    assert result["command"]["exit_code"] == 0
    assert result["capture"].sha256.startswith("sha256:")
    assert result["execution"].path.name == "execution.json"


def test_owned_capture_rejects_preexisting_output(tmp_path):
    runner = tmp_path / "runner.py"
    runner.write_text("#!/usr/bin/env python3\n")
    runner.chmod(0o755)
    output = tmp_path / "capture.json"
    output.write_text("stale")
    with pytest.raises(ValueError, match="fresh"):
        capture.execute_owned_native(
            runner,
            ["--out", str(output)],
            cwd=tmp_path,
            log_dir=tmp_path / "logs",
            timeout=5,
        )


def test_owned_capture_preserves_failed_terminal_without_output_promotion(tmp_path):
    runner = tmp_path / "runner.py"
    runner.write_text("#!/usr/bin/env python3\nimport sys\nsys.exit(7)\n")
    runner.chmod(0o755)
    output = tmp_path / "capture.json"
    logs = tmp_path / "logs"
    with pytest.raises(ValueError, match="failed with exit 7"):
        capture.execute_owned_native(
            runner, ["--out", str(output)], cwd=tmp_path, log_dir=logs, timeout=5
        )
    assert not output.exists()
    assert json.loads((logs / "execution.json").read_text())["status"] == "failed"


@pytest.mark.parametrize("aliased_inputs", [False, True])
def test_controlled_native_capture_promotes_and_replays_one_raw_family(
    tmp_path, monkeypatch, aliased_inputs
):
    """Fake producer exercises custody plumbing, not actual Linux SDK behavior."""
    manifest, policy, binaries, families = _case(tmp_path)
    daemon, config, runner = binaries
    manifest_body = json.loads(manifest.read_text())
    family = manifest_body["families"][0]
    manifest_body["families"] = [family]
    manifest.write_text(json.dumps(manifest_body))
    policy_body = json.loads(policy.read_text())
    policy_body["cohort_manifest_sha256"] = hashlib.sha256(manifest.read_bytes()).hexdigest()
    policy.write_text(json.dumps(policy_body))
    families[0]["capture"]["runtime_binding"]["decision_policy_sha256"] = hashlib.sha256(
        policy.read_bytes()
    ).hexdigest()
    (tmp_path / family["suite"]).write_text(json.dumps(families[0]["suite"]))
    output = tmp_path / family["capture"]
    output.unlink()
    template = tmp_path / "fake-runner-template.json"
    template.write_text(json.dumps(families[0]["capture"]))
    runner.write_text(
        "#!/usr/bin/env python3\n"
        "import hashlib, json, pathlib, sys\n"
        f"item=json.loads(pathlib.Path({str(template)!r}).read_text())\n"
        "image=hashlib.sha256(pathlib.Path(sys.argv[0]).read_bytes()).hexdigest()\n"
        "item['runner_binary_sha256']=image\n"
        "item['runtime_binding']['runner_binary_sha256']=image\n"
        "pathlib.Path(sys.argv[sys.argv.index('--out')+1]).write_text(json.dumps(item))\n"
    )
    runner.chmod(0o755)
    source = sample_evidence()["source"]
    monkeypatch.setattr(capture, "source_identity", lambda *_: source)
    import evidence_bridge

    monkeypatch.setattr(evidence_bridge, "source_identity", lambda *_: source)
    import benchctl

    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda *_: None)
    monkeypatch.setattr(benchctl, "require_frozen_source", lambda *_: None)
    monkeypatch.setattr(benchctl, "resolve_checkout_head", lambda *_: source["revision"])

    def fake_replay(*args):
        replay = copy.deepcopy(families[0])
        replay["capture"] = json.loads(args[3].read_text())
        return replay

    monkeypatch.setattr(evaluator, "load_native_file_replay", fake_replay)
    alias = tmp_path / "alias"
    if aliased_inputs:
        alias.symlink_to(tmp_path, target_is_directory=True)

    def planned_path(path: Path) -> str:
        return str(alias / path.name) if aliased_inputs else str(path)

    plan = {
        "schema_version": 1,
        "cohort_manifest": planned_path(manifest),
        "decision_policy": planned_path(policy),
        "daemon_binary": planned_path(daemon),
        "daemon_config": planned_path(config),
        "runner_binary": planned_path(runner),
        "family_id": family["family_id"],
        "view_json": "{}",
        "open_key": "1" * 64,
        "text_instance": "1" * 32,
        "semantic_instance": "2" * 32,
        "file_instance": "3" * 32,
        "lane_top_k": 60,
        "file_top_k": 10,
    }
    plan_path = tmp_path / "plan.json"
    plan_path.write_text(json.dumps(plan))
    root = tmp_path / "evidence"
    repo = Path(__file__).resolve().parents[3]
    registry = load_registry()
    document = capture.capture(repo, root, registry, 5, plan_path)
    assert len(document["runs"]) == 1
    record = RunStore(root).load(document["runs"][0]["run_id"])
    assert record["command"]["argv"][1] == "native-file-profile"
    capture.replay_run(RunStore(root), record)
    import registry as registry_module

    drifted_registry = copy.deepcopy(registry)
    drifted_registry["producers"][capture.FAMILY]["kind"] = "recorded-input"
    monkeypatch.setattr(registry_module, "load_registry", lambda: drifted_registry)
    with pytest.raises(ValueError, match="registration"):
        capture.replay_run(RunStore(root), record)
    monkeypatch.setattr(registry_module, "load_registry", lambda: registry)
    stale_source = {**source, "closure_digest": "sha256:" + "f" * 64}
    monkeypatch.setattr(capture, "source_identity", lambda *_: stale_source)
    with pytest.raises(ValueError, match="code source"):
        capture.replay_run(RunStore(root), record)
    monkeypatch.setattr(capture, "source_identity", lambda *_: source)
    commitments = native_file_decision.verify_controlled_capture_runs(
        manifest, policy, daemon, config, runner, root, [record["run_id"]]
    )
    assert commitments == {
        family["family_id"]: evaluator.digest(evaluator.canonical(json.loads(output.read_text())))
    }
    with pytest.raises(ValueError, match="inventory"):
        native_file_decision.verify_controlled_capture_runs(
            manifest, policy, daemon, config, runner, root, []
        )
    assert capture.validate(repo, root, registry)["capture_id"] == document["capture_id"]
    daemon.write_bytes(b"changed image")
    with pytest.raises(ValueError, match="inputs changed"):
        capture.replay_run(RunStore(root), record)
    daemon.write_bytes(b"native daemon image")
    output.write_text("{}")
    with pytest.raises(ValueError, match="output changed"):
        capture.replay_run(RunStore(root), record)
