"""Native capture orchestration tests; fake runner is not an SDK/product proof."""

import copy
import hashlib
import json
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import native_file_capture as capture
from evidence import RawFile, RunStore, sample_evidence, seal, to_canonical_json
from registry import load_registry

from tools.benchmark.retrieval import evaluator, native_file_decision
from tools.ci.tests.test_native_file_decision import _case, _selected_native
from tools.ci.tests.test_native_file_evidence import source_bound_capture


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


@pytest.fixture
def replay_paths(tmp_path):
    return capture._CapturePaths(
        files={
            key: tmp_path / f"{key}.json"
            for key in ("suite", "query_pack", "source_universe", "source_binding")
        },
        releases={key: tmp_path / key for key in ("base_release", "target_releases")},
        source_repo=tmp_path / "source",
    )


def test_replay_family_forwards_validator_and_archived_capture_commitment(
    tmp_path, monkeypatch, replay_paths
):
    archived = tmp_path / "archived-capture.json"
    archived.write_bytes(b"{}")
    captured = RawFile.capture(archived)
    def validator(path):
        return {"release": str(path)}
    forwarded = {}

    def fake_replay(*args, expected_capture, release_validator):
        forwarded["source_repo"] = args[0]
        forwarded["capture"] = args[3]
        forwarded["expected_capture"] = expected_capture
        forwarded["release_validator"] = release_validator
        return {}

    monkeypatch.setattr(evaluator, "load_native_file_execution", fake_replay)
    capture._replay_family(replay_paths, "family", captured, release_validator=validator)
    assert forwarded == {
        "source_repo": tmp_path / "source",
        "capture": archived,
        "expected_capture": (captured.sha256, captured.size),
        "release_validator": validator,
    }


@pytest.mark.parametrize("mutation", [None, "foreign_config", "changed_config", "wrong_selection", "missing_selection"])
def test_v2_plan_runtime_is_bound_to_family(tmp_path, mutation):
    manifest = tmp_path / "cohort.json"
    config = tmp_path / "daemon.json"
    config.write_bytes(b'{"socket_path":"/tmp/native.sock"}')
    selection = tmp_path / "selected.json"
    selection.write_bytes(b"selected bytes")
    family = {
        "daemon_config": config.name,
        "daemon_config_sha256": hashlib.sha256(config.read_bytes()).hexdigest(),
        "selection_path": selection.name,
        "selection_sha256": hashlib.sha256(selection.read_bytes()).hexdigest(),
    }
    plan = {
        "daemon_config": str(config),
        "selection_path": str(selection),
        "selection_sha256": family["selection_sha256"],
    }
    if mutation == "foreign_config":
        other = tmp_path / "other-daemon.json"
        other.write_bytes(config.read_bytes())
        plan["daemon_config"] = str(other)
    elif mutation == "changed_config":
        config.write_bytes(config.read_bytes() + b" ")
    elif mutation == "wrong_selection":
        plan["selection_sha256"] = "0" * 64
    elif mutation == "missing_selection":
        del plan["selection_path"], plan["selection_sha256"]
    if mutation is None:
        capture._check_plan_family_runtime(manifest, family, plan)
    else:
        with pytest.raises(ValueError, match="daemon config|selection"):
            capture._check_plan_family_runtime(manifest, family, plan)


def test_v2_snapshot_rejects_config_replacement_after_plan_check(tmp_path):
    manifest = tmp_path / "cohort.json"
    config = tmp_path / "daemon.json"
    config.write_bytes(b'{"socket_path":"/tmp/native.sock"}')
    family = {
        "daemon_config": config.name,
        "daemon_config_sha256": hashlib.sha256(config.read_bytes()).hexdigest(),
    }
    capture._check_plan_family_runtime(manifest, family, {"daemon_config": str(config)})
    config.write_bytes(config.read_bytes() + b" ")
    snapshot = capture._snapshot({"daemon_config": config}, {})
    with pytest.raises(ValueError, match="frozen cohort family"):
        capture._check_frozen_family_snapshot(family, snapshot)


def test_controlled_replay_accepts_unjudged_pools_but_checks_each_query(
    tmp_path, monkeypatch, replay_paths
):
    from tools.benchmark.retrieval import composite_source_binding, native_file_evidence
    from tools.ci.tests.test_native_file_evidence import unlabelled_capture

    suite, pack, observed, source = unlabelled_capture()
    provenance = {
        "source_universe_sha256": "a" * 64, "source_binding_sha256": "b" * 64,
    }
    suite["source_provenance"] = {"kind": "composite_git_releases_v1", **provenance}
    for task in suite["tasks"]:
        task["query_family_id"] = "family"
    pack["suite_commitment_sha256"] = evaluator.digest(evaluator.canonical(suite))
    observed.update(
        **provenance, suite_commitment_sha256=pack["suite_commitment_sha256"],
        query_pack_sha256=evaluator.digest(evaluator.canonical(pack)),
    )
    monkeypatch.setattr(evaluator, "_load_native_file_inputs", lambda *_a, **_k: (suite, pack, observed, source))
    monkeypatch.setattr(composite_source_binding, "verify", lambda *_a, **_k: {"family_id": "family"})
    archive = tmp_path / "capture.json"
    archive.write_bytes(evaluator.canonical(observed))
    stored = RawFile.capture(archive)
    assert capture._replay_family(replay_paths, "family", stored) == observed
    with pytest.raises(evaluator.EvidenceError, match="qrels are incomplete"):
        native_file_evidence.evaluate(suite, pack, observed, source, (tmp_path,) * 4 + ("family",))
    observed["tasks"][-1]["hybrid"]["view"] = {**observed["view"], "sequence": 4}
    with pytest.raises(evaluator.EvidenceError, match="view/session"):
        capture._replay_family(replay_paths, "family", stored)


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


def test_owned_capture_refuses_boolean_success_code(tmp_path, monkeypatch):
    runner = tmp_path / "runner"
    runner.write_bytes(b"runner")
    output = tmp_path / "capture.json"
    logs = tmp_path / "logs"

    def false_success(*_args, **_kwargs):
        output.write_bytes(b"{}")
        (logs / "execution.json").write_bytes(b"{}")
        return None, None, {"exit_code": False}

    monkeypatch.setattr(capture, "execute", false_success)
    with pytest.raises(capture.EvidenceError, match="successful terminal status"):
        capture.execute_owned_native(
            runner, ["--out", str(output)], cwd=tmp_path, log_dir=logs, timeout=5
        )


def test_selected_plan_requires_path_and_digest_together(tmp_path):
    plan = {key: "x" for key in capture.PLAN_KEYS}
    plan["schema_version"] = 1
    plan["selection_path"] = str(tmp_path / "selected.json")
    path = tmp_path / "plan.json"
    path.write_text(json.dumps(plan))
    with pytest.raises(ValueError, match="plan shape"):
        capture._plan(path)


def test_signed_plan_cannot_omit_the_installed_approval(tmp_path):
    plan = {key: "x" for key in capture.PLAN_KEYS | capture.SELECTION_KEYS}
    plan["schema_version"] = 2
    path = tmp_path / "plan.json"
    path.write_text(json.dumps(plan))
    with pytest.raises(ValueError, match="plan shape"):
        capture._plan(path)


@pytest.mark.parametrize(
    (
        "aliased_inputs",
        "selected",
        "replace_selection_during_run",
        "mismatched_field",
        "replay_only",
    ),
    [
        (alias, selected, replace, None, False)
        for alias in (False, True)
        for selected, replace in ((False, False), (True, False), (True, True))
    ]
    + [
        (False, True, False, field, False)
        for field in (
            "view_json",
            "text_instance",
            "semantic_instance",
            "file_instance",
            "lane_top_k",
            "file_top_k",
            "query_deadline_seconds",
            "foreign_selection_binary",
        )
    ]
    + [
        (False, False, False, "file_instance", True),
        (False, False, False, "foreign_policy_cohort", True),
        (False, False, False, "unmanaged_output", False),
        (False, False, False, "dangling_output", False),
    ]
    + [
        (False, False, False, overlap, False)
        for overlap in (
            "profile_pointer_output", "policy_as_pointer", "store_under_release", "output_under_release",
            "store_under_source_repository", "output_under_source_repository",
        )
    ],
)
def test_controlled_native_capture_promotes_and_replays_one_raw_family(
    tmp_path,
    monkeypatch,
    aliased_inputs,
    selected,
    replace_selection_during_run,
    mismatched_field,
    replay_only,
    *,
    alias_family_config=False,
    other_family_location=None,
    change_source_head_at=None,
    retarget_other_repo_after_capture=False,
    signed_install=None,
    require_signed_install=False,
):
    """Fake producer exercises custody plumbing, not actual Linux SDK behavior."""
    manifest, policy, binaries, families = _case(tmp_path)
    daemon, config, runner = binaries
    manifest_body = json.loads(manifest.read_text())
    family = manifest_body["families"][0]
    other_family = manifest_body["families"][1]
    multiple_families = bool(other_family_location or retarget_other_repo_after_capture)
    manifest_body["families"] = [family, other_family] if multiple_families else [family]
    root = tmp_path / "evidence"
    profile_pointer = root / "profiles" / f"{capture.PROFILE}.json"
    overlap = mismatched_field in {
        "profile_pointer_output", "policy_as_pointer", "store_under_release", "output_under_release",
        "store_under_source_repository", "output_under_source_repository",
    }
    if mismatched_field == "profile_pointer_output":
        profile_pointer.parent.mkdir(parents=True)
        family["capture"] = str(profile_pointer)
    elif mismatched_field == "policy_as_pointer":
        profile_pointer.parent.mkdir(parents=True)
        policy.rename(profile_pointer)
        policy = profile_pointer
    elif mismatched_field == "store_under_release":
        root = tmp_path / family["base_release"] / "evidence"
    elif mismatched_field == "output_under_release":
        family["capture"] = str(tmp_path / family["base_release"] / "new-capture.json")
    elif mismatched_field in {"store_under_source_repository", "output_under_source_repository"}:
        source_repo = tmp_path / family["repo"]
        source_repo.mkdir()
        if mismatched_field == "store_under_source_repository":
            root = source_repo / "evidence"
        else:
            family["capture"] = str(source_repo / "new-capture.json")
    elif mismatched_field == "unmanaged_output":
        export = root / "exports" / "capture.json"
        export.parent.mkdir(parents=True)
        family["capture"] = str(export)
        mismatched_field = None
    if multiple_families:
        other_base = tmp_path / other_family["base_release"]
        other_repo = tmp_path / other_family["repo"]
        other_repo.unlink(missing_ok=True)
        other_repo.mkdir(parents=True)
        subprocess.run(["git", "-C", str(other_repo), "init", "-q"], check=True)
        subprocess.run(["git", "-C", str(other_repo), "config", "user.email", "native-test@example.invalid"], check=True)
        subprocess.run(["git", "-C", str(other_repo), "config", "user.name", "Native Test"], check=True)
        (other_repo / "tracked.txt").write_text("other family source\n")
        subprocess.run(["git", "-C", str(other_repo), "add", "tracked.txt"], check=True)
        subprocess.run(["git", "-C", str(other_repo), "commit", "-qm", "Other family source"], check=True)
        if other_family_location == "store_under_release":
            root = other_base / "evidence"
        elif other_family_location == "output_under_release":
            family["capture"] = str(other_base / "native-capture.json")
        elif other_family_location == "store_under_repo":
            root = other_repo / "evidence"
        elif other_family_location == "output_under_repo":
            family["capture"] = str(other_repo / "native-capture.json")
        elif other_family_location is not None:
            raise AssertionError(other_family_location)
    v2_family = not any((aliased_inputs, mismatched_field, replay_only))
    artifact = families[0]["capture"]
    _, _, sdk_capture, *_ = source_bound_capture()
    for field in ("view", "text_instance", "semantic_instance", "indexed_universe"):
        artifact[field] = sdk_capture[field]
    artifact.update(
        lane_top_k=60,
        file_top_k=10,
        query_deadline_seconds=59 if mismatched_field == "query_deadline_seconds" else 60,
    )
    selection_path = None
    selection_bytes = None
    if selected:
        from tools.deployment import launcher

        selection_path, _seed_sha, seed = _selected_native(tmp_path)
        config.write_text(
            json.dumps(
                {"state_root": str(seed.state_root), "socket_path": str(tmp_path / "native.sock")}
            )
        )
        config.chmod(0o600)
        daemon.chmod(0o700)
        chosen = launcher.proposed_native_selection(
            daemon, seed.state_root, config, {}, seed.native_boot_receipt
        )
        selection_path.write_text(json.dumps(chosen.to_wire(), sort_keys=True) + "\n")
        selection_bytes = selection_path.read_bytes()
        binding = families[0]["capture"]["runtime_binding"]
        binding["daemon_config_sha256"] = hashlib.sha256(config.read_bytes()).hexdigest()
        binding["command"] = "serve-native"
        binding["boot_identity"] = {
            "root_nonce_hex": chosen.native_boot_receipt.root_nonce_hex,
            "manifest_operation": json.loads(chosen.native_boot_receipt.manifest_operation_json),
            "manifest_object_hex": chosen.native_boot_receipt.manifest_object_hex,
            "source_cut_digest_hex": chosen.native_boot_receipt.source_cut_digest_hex,
        }
        if multiple_families:
            other_state = tmp_path / "other-native-state"
            other_state.mkdir()
            other_config = tmp_path / "other-native-config.json"
            other_config.write_text(json.dumps({
                "state_root": str(other_state),
                "socket_path": str(tmp_path / "other-native.sock"),
            }))
            other_config.chmod(0o600)
            other_selected = launcher.proposed_native_selection(
                daemon, other_state, other_config, {}, chosen.native_boot_receipt,
            )
            other_selection = tmp_path / "other-native-selection.json"
            other_selection.write_text(json.dumps(other_selected.to_wire(), sort_keys=True) + "\n")
            other_selection.chmod(0o600)
            other_family.update(
                daemon_config=other_config.name,
                daemon_config_sha256=hashlib.sha256(other_config.read_bytes()).hexdigest(),
                selection_path=other_selection.name,
                selection_sha256=hashlib.sha256(other_selection.read_bytes()).hexdigest(),
            )
    if v2_family:
        manifest_body["schema_version"] = 2
        declared_config = config
        if alias_family_config:
            declared_config = config.with_suffix(".alias.json")
            declared_config.symlink_to(config)
        family["daemon_config"] = declared_config.name
        family["daemon_config_sha256"] = hashlib.sha256(config.read_bytes()).hexdigest()
        if selected:
            family["selection_path"] = selection_path.name
            family["selection_sha256"] = hashlib.sha256(selection_bytes).hexdigest()
    manifest.write_text(json.dumps(manifest_body))
    policy_body = json.loads(policy.read_text())
    policy_body["cohort_manifest_sha256"] = hashlib.sha256(manifest.read_bytes()).hexdigest()
    if mismatched_field == "foreign_policy_cohort":
        policy_body["cohort_manifest_sha256"] = "0" * 64
    policy.write_text(json.dumps(policy_body))
    artifact["runtime_binding"]["decision_policy_sha256"] = hashlib.sha256(
        policy.read_bytes()
    ).hexdigest()
    suite_path = tmp_path / family["suite"]
    suite_path.write_text(json.dumps(families[0]["suite"]))
    if change_source_head_at is not None:
        source_repo = tmp_path / family["repo"]
        source_repo.unlink(missing_ok=True)
        source_repo.mkdir(parents=True)
        subprocess.run(["git", "-C", str(source_repo), "init", "-q"], check=True)
        subprocess.run(["git", "-C", str(source_repo), "config", "user.email", "native-test@example.invalid"], check=True)
        subprocess.run(["git", "-C", str(source_repo), "config", "user.name", "Native Test"], check=True)
        (source_repo / "tracked.txt").write_text("original source\n")
        subprocess.run(["git", "-C", str(source_repo), "add", "tracked.txt"], check=True)
        subprocess.run(["git", "-C", str(source_repo), "commit", "-qm", "Original source"], check=True)
        frozen_head = subprocess.check_output(["git", "-C", str(source_repo), "rev-parse", "HEAD"], text=True).strip()
        suite = json.loads(suite_path.read_bytes())
        suite["repository_commit"] = frozen_head
        suite_path.write_text(json.dumps(suite))
        artifact["repository_commit"] = frozen_head
        assert evaluator.verify_repo(source_repo, frozen_head) == source_repo.resolve()
    if aliased_inputs:
        source_suite = suite_path.rename(suite_path.with_suffix(".source.json"))
        suite_path.symlink_to(source_suite)
        source_repo = tmp_path / family["repo"]
        original_repo = source_repo.with_suffix(".source")
        original_repo.mkdir()
        source_repo.symlink_to(original_repo, target_is_directory=True)
    output = tmp_path / family["capture"]
    output.unlink(missing_ok=True)
    if mismatched_field == "dangling_output":
        output.symlink_to(tmp_path / "unexpected-capture.json")
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
        + (
            f"pathlib.Path({str(selection_path)!r}).write_bytes(pathlib.Path({str(selection_path)!r}).read_bytes()+b' ')\n"
            if replace_selection_during_run
            else ""
        )
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

    forwarded_replays = []

    def fake_replay(*args, expected_capture, release_validator=None, **expected_inputs):
        forwarded_replays.append((args[3], expected_capture, release_validator))
        data = args[3].read_bytes()
        if ("sha256:" + hashlib.sha256(data).hexdigest(), len(data)) != expected_capture:
            raise ValueError("Native capture bytes differ from the controlled execution commitment")
        for key, path in (
            ("suite", args[1]),
            ("query_pack", args[2]),
            ("source_universe", args[4][0]),
            ("source_binding", args[4][1]),
        ):
            if f"expected_{key}" in expected_inputs:
                assert (RawFile.capture(path).sha256, path.stat().st_size) == expected_inputs[
                    f"expected_{key}"
                ]
        return json.loads(data)

    monkeypatch.setattr(evaluator, "load_native_file_execution", fake_replay)
    real_verify_repo = evaluator.verify_repo
    if change_source_head_at is None:
        monkeypatch.setattr(evaluator, "verify_repo", lambda repo, _commit: Path(repo).resolve())
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
        "view_json": json.dumps(artifact["view"], sort_keys=True),
        "open_key": "1" * 64,
        "text_instance": bytes(artifact["text_instance"]).hex(),
        "semantic_instance": bytes(artifact["semantic_instance"]).hex(),
        "file_instance": bytes(artifact["indexed_universe"]["file_instance"]).hex(),
        "lane_top_k": 60,
        "file_top_k": 10,
    }
    plan_overrides = {
        "view_json": json.dumps(dict(artifact["view"], digest=[3] * 32)),
        "text_instance": "aa" * 16,
        "semantic_instance": "bb" * 16,
        "file_instance": "cc" * 16,
        "lane_top_k": 61,
        "file_top_k": 11,
    }
    if mismatched_field in plan_overrides:
        plan[mismatched_field] = plan_overrides[mismatched_field]
    if mismatched_field == "foreign_selection_binary":
        foreign = tmp_path / "foreign-daemon"
        foreign.write_bytes(daemon.read_bytes())
        plan["daemon_binary"] = str(foreign)
    if selected:
        plan["selection_path"] = str(selection_path)
        plan["selection_sha256"] = hashlib.sha256(selection_bytes).hexdigest()
    if signed_install is not None:
        assert selected
        plan["schema_version"] = 2
        plan[capture.SIGNED_INSTALL] = {"fixture": "original installation"}
        approved_policy_sha = hashlib.sha256(policy.read_bytes()).hexdigest()
        installation_changed = False

        def verify_approval(approval, selection, selection_sha, policy_sha):
            assert approval == plan[capture.SIGNED_INSTALL]
            assert selection == chosen
            assert selection_sha == plan["selection_sha256"]
            assert policy_sha == approved_policy_sha
            if installation_changed:
                raise ValueError("Native release approval installation changed")

        monkeypatch.setattr(capture.native_release_approval, "verify_installed_approval", verify_approval)
        if signed_install in ("changed_during_commit", "policy_changed_during_commit", "elf_changed_during_commit"):
            original_load = RunStore.load

            def reload_then_change_install(store, run_id, *, expected_digest=None):
                nonlocal installation_changed
                record = original_load(store, run_id, expected_digest=expected_digest)
                if capture.current_capture().state["phase"] == "commit":
                    if signed_install == "policy_changed_during_commit":
                        policy.write_bytes(policy.read_bytes() + b"\n")
                    elif signed_install == "elf_changed_during_commit":
                        daemon.write_bytes(b"replaced Q ELF")
                    else:
                        installation_changed = True
                return record

            monkeypatch.setattr(RunStore, "load", reload_then_change_install)
    plan_path = tmp_path / "plan.json"
    plan_path.write_text(json.dumps(plan))
    repo = Path(__file__).resolve().parents[3]
    registry = load_registry()
    if require_signed_install and signed_install is None:
        with pytest.raises(ValueError, match="approved v2 execution plan"):
            capture.capture(repo, root, registry, 5, plan_path, require_signed_install=True)
        assert not output.exists()
        assert not (root / "work").exists()
        return
    commit_refusals = {
        "changed_during_commit": "approval installation changed",
        "policy_changed_during_commit": "Native frozen inputs changed before publication",
        "elf_changed_during_commit": "Native frozen inputs changed before publication",
    }
    if signed_install in commit_refusals:
        with pytest.raises(ValueError, match=commit_refusals[signed_install]):
            capture.capture(repo, root, registry, 5, plan_path)
        assert not profile_pointer.exists()
        assert not list((root / "captures").glob("*.json"))
        return
    if other_family_location:
        other_base = tmp_path / other_family["base_release"]
        other_repo = tmp_path / other_family["repo"]
        original_base_digest = native_file_decision._release_tree_sha(other_base)
        original_head = subprocess.check_output(["git", "-C", str(other_repo), "rev-parse", "HEAD"], text=True).strip()
        assert real_verify_repo(other_repo, original_head) == other_repo.resolve()
        with pytest.raises(ValueError, match="overlap|other family"):
            capture.capture(repo, root, registry, 5, plan_path)
        assert native_file_decision._release_tree_sha(other_base) == original_base_digest
        assert real_verify_repo(other_repo, original_head) == other_repo.resolve()
        assert not output.exists()
        assert not (root / "profiles" / f"{capture.PROFILE}.json").exists()
        return
    if change_source_head_at is not None:
        def change_head():
            (source_repo / "tracked.txt").write_text("replacement source\n")
            subprocess.run(["git", "-C", str(source_repo), "add", "tracked.txt"], check=True)
            subprocess.run(["git", "-C", str(source_repo), "commit", "-qm", "Replacement source"], check=True)
            with pytest.raises(ValueError, match="HEAD differs"):
                evaluator.verify_repo(source_repo, frozen_head)

        if change_source_head_at == "domain_replay":
            original_publish = capture.publish_capture

            def publish_after_source_head_change(*args, replay, **kwargs):
                def replay_then_change_head(store, record):
                    commitment = replay(store, record)
                    change_head()
                    return commitment

                return original_publish(*args, replay=replay_then_change_head, **kwargs)

            monkeypatch.setattr(capture, "publish_capture", publish_after_source_head_change)
        else:
            original_load = RunStore.load
            commit_loads = 0

            def reload_then_change_head(store, run_id, *, expected_digest=None):
                nonlocal commit_loads
                record = original_load(store, run_id, expected_digest=expected_digest)
                if capture.current_capture().state["phase"] == "commit":
                    commit_loads += 1
                    if commit_loads == (1 if change_source_head_at == "commit_reload" else 2):
                        change_head()
                return record

            monkeypatch.setattr(RunStore, "load", reload_then_change_head)
        with pytest.raises(ValueError, match="source repository|checkout HEAD|frozen inputs"):
            capture.capture(repo, root, registry, 5, plan_path)
        assert not profile_pointer.exists()
        assert not list((root / "captures").glob("*.json"))
        return
    if mismatched_field == "dangling_output":
        with pytest.raises(ValueError, match="fresh absolute path"):
            capture.capture(repo, root, registry, 5, plan_path)
        assert output.is_symlink() and not output.exists()
        assert not (root / "work").exists()
        return
    if overlap:
        original_policy = policy.read_bytes()
        with pytest.raises(ValueError, match="Native .*overlap"):
            capture.capture(repo, root, registry, 5, plan_path)
        assert not output.exists()
        assert not (root / "work").exists()
        assert policy.read_bytes() == original_policy
        return
    if replace_selection_during_run:
        with pytest.raises(ValueError, match="selection|frozen inputs changed"):
            capture.capture(repo, root, registry, 5, plan_path)
        return
    if replay_only:
        error = f"execution plan.*{mismatched_field}"
        with monkeypatch.context() as legacy:
            if mismatched_field == "foreign_policy_cohort":
                def hash_without_cohort_check(policy_input, *_):
                    raw = capture.read_control(policy_input)
                    return {}, hashlib.sha256(raw).hexdigest()

                legacy.setattr(capture.native_file_decision, "_policy", hash_without_cohort_check)
                error = "decision policy or frozen cohort differs"
            else:
                legacy.setattr(capture, "_check_plan_capture", lambda *_: None)
            document = capture.capture(repo, root, registry, 5, plan_path)
        store = RunStore(root)
        record = store.load(document["runs"][0]["run_id"])
        with pytest.raises(ValueError, match=error):
            capture.replay_run(store, record)
        return
    if mismatched_field == "foreign_selection_binary":
        with pytest.raises(ValueError, match="selection binary path"):
            capture.capture(repo, root, registry, 5, plan_path)
        assert not output.exists()
        assert not list(RunStore(root).runs_dir.glob("*"))
        return
    if mismatched_field:
        with pytest.raises(ValueError, match=f"execution plan.*{mismatched_field}"):
            capture.capture(repo, root, registry, 5, plan_path)
        assert not list(RunStore(root).runs_dir.glob("*"))
        return
    document = capture.capture(
        repo, root, registry, 5, plan_path, require_signed_install=require_signed_install,
    )
    if signed_install == "stable":
        run_id = document["runs"][0]["run_id"]
        strict_args = ["--evidence-root", str(root), "--require-signed-install"]
        assert benchctl.main(["validate", capture.PROFILE, *strict_args]) == 0
        assert benchctl.main(["summarize", capture.PROFILE, *strict_args]) == 0
        assert benchctl.main(["replay", run_id, *strict_args]) == 0
        publication = root / "captures" / f"{document['capture_id']}.json"
        publication.rename(publication.with_suffix(".withheld"))
        assert benchctl.main(["replay", run_id, *strict_args]) == 2
        return
    if retarget_other_repo_after_capture:
        store = RunStore(root)
        record = store.load(document["runs"][0]["run_id"])
        assert capture.replay_run(store, record)
        cohort_bytes = manifest.read_bytes()
        captured_paths = capture._roles(plan_path, plan, manifest, family)
        captured_snapshot = capture._snapshot(captured_paths.files, captured_paths.releases)
        pointer = root / "profiles" / f"{capture.PROFILE}.json"
        original_pointer = pointer.read_bytes()
        other_repo = tmp_path / other_family["repo"]
        original_repo = other_repo.with_suffix(".original")
        other_repo.rename(original_repo)
        other_repo.symlink_to(root, target_is_directory=True)
        assert manifest.read_bytes() == cohort_bytes
        assert capture._snapshot(captured_paths.files, captured_paths.releases) == captured_snapshot
        with pytest.raises(ValueError, match="overlap"):
            capture.replay_run(store, record)
        assert pointer.read_bytes() == original_pointer
        return
    assert len(document["runs"]) == 1
    record = RunStore(root).load(document["runs"][0]["run_id"])
    assert record["command"]["argv"][1] == "native-file-profile"
    store = RunStore(root)
    capture.replay_run(store, record)
    if not selected and not aliased_inputs:
        assert forwarded_replays[-1][2] is None

        def validator(path):
            return {"release": str(path)}

        capture.replay_run(store, record, release_validator=validator)
        archived_capture = next(
            row for row in record["raw"] if row["path"] == "raw/native-capture.json"
        )
        assert forwarded_replays[-1] == (
            store.run_dir(record["run_id"]) / "raw" / "native-capture.json",
            (archived_capture["sha256"], archived_capture["bytes"]),
            validator,
        )
        rejected = copy.deepcopy(record)
        rejected["verdict"].update(status="not_run", reason="no admissible execution")
        evidence_path = store.run_dir(record["run_id"]) / "evidence.json"
        original_evidence = evidence_path.read_bytes()
        try:
            evidence_path.write_text(to_canonical_json(seal(rejected)))
            assert benchctl.replay_command(repo, record["run_id"], root) == 2
        finally:
            evidence_path.write_bytes(original_evidence)
    with monkeypatch.context() as relative_replay:
        relative_replay.chdir(tmp_path)
        capture.replay_run(RunStore(Path("evidence")), record)
    raw = store.run_dir(record["run_id"]) / "raw"
    if not selected and not aliased_inputs:
        evidence_path = store.run_dir(record["run_id"]) / "evidence.json"
        original_evidence = evidence_path.read_bytes()
        with monkeypatch.context() as during_validate:
            original_load_capture = capture.load_capture

            def swap_after_manifest_load(*args, **kwargs):
                loaded = original_load_capture(*args, **kwargs)
                changed = copy.deepcopy(record)
                changed["payload"]["execution_context_digest"] = "sha256:" + "f" * 64
                evidence_path.write_text(to_canonical_json(seal(changed)))
                return loaded

            during_validate.setattr(capture, "load_capture", swap_after_manifest_load)
            try:
                with pytest.raises(ValueError, match="expected capture reference"):
                    capture.validate(repo, root, registry)
            finally:
                evidence_path.write_bytes(original_evidence)

        suite_path = (manifest.parent / family["suite"]).resolve()
        archived_suite = raw / "suite.json"
        original_suite = suite_path.read_bytes()
        original_archived_suite = archived_suite.read_bytes()
        revised = copy.deepcopy(record)
        try:
            suite_path.write_bytes(original_suite + b" ")
            archived_suite.write_bytes(original_archived_suite + b" ")
            for entry in revised["inputs"]:
                if entry["id"] == "suite":
                    entry["digest"] = RawFile.capture(suite_path).sha256
            with pytest.raises(ValueError, match="archived suite"):
                capture.replay_run(store, revised)
        finally:
            suite_path.write_bytes(original_suite)
            archived_suite.write_bytes(original_archived_suite)

        for path, error in (
            (policy, "source inputs changed during replay"),
            (output, "capture output changed"),
        ):
            original = path.read_bytes()
            with monkeypatch.context() as during_replay:
                original_replay = evaluator.load_native_file_execution

                def replace_after_replay(*args, target=path, before=original, replay=original_replay, **kwargs):
                    result = replay(*args, **kwargs)
                    target.write_bytes(before + b" ")
                    return result

                during_replay.setattr(evaluator, "load_native_file_execution", replace_after_replay)
                try:
                    with pytest.raises(ValueError, match=error):
                        capture.replay_run(store, record)
                finally:
                    path.write_bytes(original)

        execution_path = raw / "execution.json"
        original_execution = execution_path.read_bytes()
        original_evidence = evidence_path.read_bytes()
        for mutation in ("false_exit", "null_request", "malformed_raw", "false_stream_size"):
            changed = json.loads(original_execution)
            if mutation == "false_exit":
                changed["command"]["exit_code"] = False
            elif mutation == "null_request":
                changed["request"] = None
            elif mutation == "malformed_raw":
                changed["raw"][0] = None
            elif mutation == "false_stream_size":
                assert changed["raw"][1]["bytes"] == 0
                changed["raw"][1]["bytes"] = False
            try:
                execution_path.write_text(json.dumps(changed))
                revised = copy.deepcopy(record)
                reference = next(
                    row for row in revised["raw"] if row["path"] == "raw/execution.json"
                )
                archived = RawFile.capture(execution_path)
                reference["sha256"], reference["bytes"] = archived.sha256, archived.size
                evidence_path.write_text(to_canonical_json(seal(revised)))
                loaded = store.load(record["run_id"])
                with pytest.raises(capture.EvidenceError, match="controlled execution"):
                    capture.replay_run(store, loaded)
            finally:
                execution_path.write_bytes(original_execution)
                evidence_path.write_bytes(original_evidence)
    for error, changed_paths in (
        ("output changed", (raw / "native-capture.json", output)),
        ("controlled execution commitment", (raw / "native-capture.json",)),
        ("prepared file commitment", (raw / "plan.json",)),
        ("prepared file commitment", (raw / "execution.json",)),
        ("producer stream differs", (raw / "stdout",)),
        ("producer stream differs", (raw / "stderr",)),
    ):
        originals = [path.read_bytes() for path in changed_paths]
        try:
            for path, original in zip(changed_paths, originals, strict=True):
                path.write_bytes(original + b" ")
            with pytest.raises(ValueError, match=error):
                capture.replay_run(store, record)
        finally:
            for path, original in zip(changed_paths, originals, strict=True):
                path.write_bytes(original)

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
        manifest, policy, daemon, None if v2_family else config, runner, root, [record["run_id"]]
    )
    assert commitments == {
        family["family_id"]: evaluator.digest(evaluator.canonical(json.loads(output.read_text())))
    }
    if v2_family:
        scored_capture = RawFile.capture(output)
        input_digests = {row["id"]: row["digest"] for row in record["inputs"] if row["id"] != "plan"}
        args = (manifest, policy, daemon, None, runner, root, [record["run_id"]])
        exact = native_file_decision._ScoredFamilyInputs(
            (scored_capture.sha256, scored_capture.size), input_digests,
        )
        assert native_file_decision.verify_controlled_capture_runs(
            *args, scored_inputs={family["family_id"]: exact},
        ) == commitments

        reformatted = output.read_bytes() + b"\n"
        assert json.loads(reformatted) == json.loads(output.read_bytes())
        different_capture = native_file_decision._ScoredFamilyInputs(
            ("sha256:" + hashlib.sha256(reformatted).hexdigest(), len(reformatted)),
            input_digests,
        )
        different_cohort = native_file_decision._ScoredFamilyInputs(
            exact.capture, {**input_digests, "cohort": "sha256:" + "a" * 64},
        )
        for observed, error in (
            (different_capture, "raw bytes differ from scored capture"),
            (different_cohort, "inputs differ from scored family"),
        ):
            with pytest.raises(ValueError, match=error):
                native_file_decision.verify_controlled_capture_runs(
                    *args, scored_inputs={family["family_id"]: observed},
                )
    if selected:
        selection_path.write_bytes(selection_bytes + b" ")
        with pytest.raises(ValueError, match="selection|inputs changed"):
            capture.replay_run(RunStore(root), record)
        selection_path.write_bytes(selection_bytes)
    with pytest.raises(ValueError, match="inventory"):
        native_file_decision.verify_controlled_capture_runs(
            manifest, policy, daemon, None if v2_family else config, runner, root, []
        )
    assert capture.validate(repo, root, registry)["capture_id"] == document["capture_id"]
    daemon.write_bytes(b"changed image")
    with pytest.raises(ValueError, match="inputs changed"):
        capture.replay_run(RunStore(root), record)
    daemon.write_bytes(b"native daemon image")
    output.write_text("{}")
    with pytest.raises(ValueError, match="output changed"):
        capture.replay_run(RunStore(root), record)


@pytest.mark.parametrize(
    "drift_target",
    ["policy", "output", "suite_alias", "daemon_config_alias", "repo_alias", "repo_alias_during_hash"],
)
def test_native_capture_refuses_input_drift_after_domain_replay(tmp_path, monkeypatch, drift_target):
    original_publish = capture.publish_capture

    def publish_with_drift(*args, replay, **kwargs):
        def replay_then_drift(store, record):
            result = replay(store, record)
            if drift_target.startswith("repo_alias"):
                family = json.loads((tmp_path / "cohort.json").read_bytes())["families"][0]
                repo = tmp_path / family["repo"]
                assert repo.is_symlink()
                alternate = repo.with_suffix(".replacement")
                alternate.mkdir()

                def retarget_repo():
                    repo.unlink()
                    repo.symlink_to(alternate, target_is_directory=True)

                if drift_target == "repo_alias_during_hash":
                    original_snapshot = capture._snapshot

                    def snapshot_then_retarget(files, trees):
                        result = original_snapshot(files, trees)
                        retarget_repo()
                        return result

                    monkeypatch.setattr(capture, "_snapshot", snapshot_then_retarget)
                else:
                    retarget_repo()
            elif drift_target in {"suite_alias", "daemon_config_alias"}:
                family = json.loads((tmp_path / "cohort.json").read_bytes())["families"][0]
                key = "suite" if drift_target == "suite_alias" else "daemon_config"
                alias = tmp_path / family[key]
                assert alias.is_symlink()
                alternate = alias.with_suffix(".replacement.json")
                alternate.write_bytes(alias.read_bytes() + (b" " if key == "suite" else b""))
                alternate.chmod(0o600)
                alias.unlink()
                alias.symlink_to(alternate)
            else:
                path = tmp_path / "policy.json" if drift_target == "policy" else Path(record["command"]["argv"][-1])
                path.write_bytes(path.read_bytes() + b" ")
            return result

        return original_publish(*args, replay=replay_then_drift, **kwargs)

    monkeypatch.setattr(capture, "publish_capture", publish_with_drift)
    with pytest.raises(
        ValueError,
        match="frozen inputs changed before publication|capture output changed|daemon config differs from cohort family",
    ):
        test_controlled_native_capture_promotes_and_replays_one_raw_family(
            tmp_path, monkeypatch,
            drift_target == "suite_alias" or drift_target.startswith("repo_alias"),
            drift_target == "daemon_config_alias",
            False, None, False, alias_family_config=drift_target == "daemon_config_alias",
        )
    assert not (tmp_path / "evidence" / "profiles" / f"{capture.PROFILE}.json").exists()


@pytest.mark.parametrize(
    ("target", "message"),
    [("policy", "frozen inputs changed before publication"), ("output", "capture output changed")],
)
def test_native_capture_refuses_drift_during_final_runstore_read(tmp_path, monkeypatch, target, message):
    original_load = RunStore.load
    commit_reads = 0

    def load_then_change_input(store, run_id, *, expected_digest=None):
        nonlocal commit_reads
        record = original_load(store, run_id, expected_digest=expected_digest)
        if commit_reads < 2 and capture.current_capture().state["phase"] == "commit":
            commit_reads += 1
            if commit_reads == 2:
                path = (
                    tmp_path / "policy.json"
                    if target == "policy" else Path(record["command"]["argv"][-1])
                )
                path.write_bytes(path.read_bytes() + b"\n")
        return record

    monkeypatch.setattr(RunStore, "load", load_then_change_input)
    with pytest.raises(ValueError, match=message):
        test_controlled_native_capture_promotes_and_replays_one_raw_family(
            tmp_path, monkeypatch, False, True, False, None, False,
        )
    assert commit_reads == 2
    root = tmp_path / "evidence"
    assert not (root / "profiles" / f"{capture.PROFILE}.json").exists()
    assert not list((root / "captures").glob("*.json"))


@pytest.mark.parametrize(
    "location",
    ("store_under_release", "output_under_release", "store_under_repo", "output_under_repo"),
)
def test_selected_capture_protects_other_family_source_and_release(
    tmp_path, monkeypatch, location,
):
    test_controlled_native_capture_promotes_and_replays_one_raw_family(
        tmp_path, monkeypatch, False, True, False, None, False,
        other_family_location=location,
    )


@pytest.mark.parametrize("boundary", ["domain_replay", "commit_reload", "manifest_reload"])
def test_selected_capture_refuses_source_head_changed_before_publication(tmp_path, monkeypatch, boundary):
    test_controlled_native_capture_promotes_and_replays_one_raw_family(
        tmp_path, monkeypatch, False, True, False, None, False,
        change_source_head_at=boundary,
    )


def test_selected_replay_refuses_other_family_repo_retarget_to_published_store(
    tmp_path, monkeypatch,
):
    test_controlled_native_capture_promotes_and_replays_one_raw_family(
        tmp_path, monkeypatch, False, True, False, None, False,
        retarget_other_repo_after_capture=True,
    )


@pytest.mark.parametrize("drift", ["changed_during_commit", "policy_changed_during_commit",
                                   "elf_changed_during_commit"])
def test_signed_capture_refuses_installation_changed_during_commit(tmp_path, monkeypatch, drift):
    test_controlled_native_capture_promotes_and_replays_one_raw_family(
        tmp_path, monkeypatch, False, True, False, None, False,
        signed_install=drift,
    )


def test_signed_capture_refuses_selected_unsigned_plan_before_admission(tmp_path, monkeypatch):
    test_controlled_native_capture_promotes_and_replays_one_raw_family(
        tmp_path, monkeypatch, False, True, False, None, False, require_signed_install=True,
    )


def test_signed_native_revalidates_only_a_published_approved_run(tmp_path, monkeypatch, capsys):
    test_controlled_native_capture_promotes_and_replays_one_raw_family(
        tmp_path, monkeypatch, False, True, False, None, False,
        signed_install="stable", require_signed_install=True,
    )
    observed = capsys.readouterr()
    assert '"signed_install_verified": true' in observed.out
    assert "replay refused" in observed.err
