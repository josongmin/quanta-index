"""Independent P11 transition fixtures; no Linux operational qualification."""

from __future__ import annotations

import copy
import hashlib
import json
import re
from pathlib import Path

import pytest

from tools.ci import operational_proof as cli
from tools.ci import proof_operational_result as module


def digest(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def install_contract(root: Path, proof_id: str, host: str = "sha256:" + "3" * 64) -> str:
    """Prepare source-owned fixture actors before a fixture repository commit."""
    actors = {phase: "tools/ci/fixtures/operation_" + phase + ".py" for phase in module.PHASES}
    for phase, name in actors.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"# Independent {phase} fixture actor.\n", encoding="utf-8")
    binary = root / "bin/searchd"
    binary_digest = digest(binary.read_bytes() if binary.exists() else b"release-daemon")
    expected = {
        "daemon_sha256": binary_digest,
        "config_sha256": "2" * 64,
        "state_root_format": "v2",
    }
    if proof_id == "p11-activation":
        expected.update(generation="generation-after", query_result_sha256="4" * 64)
    elif proof_id == "p11-rollback":
        expected.update(
            backup_root_incarnation="backup-incarnation",
            backup_data_sha256="5" * 64,
            sequence_high_water=41,
        )
    contract = {
        "schema_version": 1,
        "proof_id": proof_id,
        "timeout_seconds": 10,
        "actors": actors,
        "expected": expected,
        "target": {
            "host_identity_digest": host,
            "binary_path": "/opt/quanta/bin/searchd",
            "config_path": "/etc/quanta/config",
            "state_root": "/var/lib/quanta/state",
        },
    }
    relative = f"tools/ci/fixtures/{proof_id}.json"
    (root / relative).write_text(json.dumps(contract), encoding="utf-8")
    return relative


def fixture_binding(contract: dict) -> dict:
    return {
        "source": {"head": "a" * 40, "dirty_digest": "sha256:" + "0" * 64},
        "source_pair": {"source": {"head": "b" * 40}},
        "daemon_sha256": contract["expected"]["daemon_sha256"],
        "host_identity_digest": contract["target"]["host_identity_digest"],
        "state_root_format": contract["expected"]["state_root_format"],
        "dependency_receipts": [],
    }


def fixture_states(contract: dict) -> tuple[dict, dict]:
    expected = contract["expected"]
    base = {key: expected[key] for key in module.BASE_STATE}
    action = module.ACTIONS[contract["proof_id"]]
    if action == "deployment":
        return {
            "daemon_sha256": "0" * 64,
            "config_sha256": "0" * 64,
            "state_root_format": "v1",
        }, base
    if action == "activation":
        return dict(base, generation="generation-before", query_result_sha256="0" * 64), dict(
            base,
            generation=expected["generation"],
            query_result_sha256=expected["query_result_sha256"],
        )
    return dict(
        base, root_incarnation="live-before", data_sha256="0" * 64, sequence_high_water=99
    ), dict(
        base,
        root_incarnation="restored-new",
        data_sha256=expected["backup_data_sha256"],
        sequence_high_water=expected["sequence_high_water"],
    )


def archive(root: Path, relative: str) -> dict[str, str]:
    raw = (root / relative).read_bytes()
    sha = digest(raw)
    path = f"artifacts/proof-authority/evidence/{sha}"
    destination = root / path
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(raw)
    return {"source_path": relative, "path": path, "sha256": sha}


def fixture_evidence(root: Path, proof: dict, binding: dict) -> tuple[dict, list[dict]]:
    """Fixed typed raw observations for manifest/aggregate owner tests only."""
    relative = proof["operational_contract"]
    contract = json.loads((root / relative).read_text())
    pre, post = fixture_states(contract)
    run_id = digest(proof["id"].encode())[:32]
    record = {
        "schema_version": 1,
        "proof_id": proof["id"],
        "run_id": run_id,
        "binding": binding,
        "contract_sha256": digest((root / relative).read_bytes()),
        "steps": [],
    }
    sources = {relative, *contract["actors"].values()}
    for index, phase in enumerate(module.PHASES):
        prefix = f"artifacts/proof-authority/raw/{proof['id']}/{phase}"
        request = {
            "schema_version": 1,
            "proof_id": proof["id"],
            "run_id": run_id,
            "phase": phase,
            "target": contract["target"],
        }
        if phase == "action":
            request["expected"] = contract["expected"]
        observed = pre if phase == "pre" else post
        if phase == "action":
            observed = {"action": module.ACTIONS[proof["id"]], "completed": True}
        event = {key: value for key, value in request.items() if key != "expected"}
        event["observed"] = observed
        owner = contract["actors"][phase]
        paths = {key: prefix + "." + key for key in ("stdout", "stderr", "request", "execution")}
        argv = [
            "/usr/bin/python3",
            "-B",
            str(root / owner),
            "--request",
            str(root / paths["request"]),
        ]
        for key, raw in (
            ("stdout", json.dumps(event).encode()),
            ("stderr", b""),
            ("request", json.dumps(request).encode()),
        ):
            file = root / paths[key]
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(raw)
        execution = {
            "command": {
                "argv": argv,
                "cwd": str(root),
                "status": "completed",
                "exit_code": 0,
                "timeout_seconds": contract["timeout_seconds"],
                "wall_ms": 1,
            },
            "request": {
                "argv": argv,
                "cwd": str(root),
                "timeout_seconds": contract["timeout_seconds"],
            },
            "status": "completed",
            "error_type": None,
            "output_errors": [],
            "raw": [
                {
                    "path": str(root / paths[key]),
                    "sha256": "sha256:" + digest((root / paths[key]).read_bytes()),
                    "bytes": len((root / paths[key]).read_bytes()),
                }
                for key in ("stdout", "stderr")
            ],
        }
        (root / paths["execution"]).write_text(json.dumps(execution))
        record["steps"].append(
            {
                "phase": phase,
                "owner": owner,
                "argv": argv,
                "owner_sha256": digest((root / owner).read_bytes()),
                "started_at": f"2026-09-21T00:00:0{index * 2}+00:00",
                "ended_at": f"2026-09-21T00:00:0{index * 2 + 1}+00:00",
                **paths,
            }
        )
        sources.update(paths.values())
    event_path = f"artifacts/proof-authority/raw/{proof['id']}/events.json"
    (root / event_path).write_text(json.dumps(record))
    sources.add(event_path)
    return {
        "schema_version": 1,
        "kind": "operational",
        "contract": relative,
        "events": event_path,
        "target": module.target_identity(contract),
    }, [archive(root, name) for name in sorted(sources)]


@pytest.fixture
def evidence(tmp_path: Path):
    path = install_contract(tmp_path, "p11-deployment")
    proof = {
        "id": "p11-deployment",
        "execution_mode": "operational-action",
        "authority_state": "executable",
        "operational_contract": path,
    }
    binding = fixture_binding(json.loads((tmp_path / path).read_text()))
    result, artifacts = fixture_evidence(tmp_path, proof, binding)
    return tmp_path, proof, binding, result, artifacts


@pytest.mark.parametrize("proof_id", tuple(module.ACTIONS))
def test_complete_observed_action_is_derived_as_one_action(tmp_path: Path, proof_id: str):
    path = install_contract(tmp_path, proof_id)
    proof = {"id": proof_id, "execution_mode": "operational-action", "operational_contract": path}
    binding = fixture_binding(json.loads((tmp_path / path).read_text()))
    result, artifacts = fixture_evidence(tmp_path, proof, binding)
    assert (
        module.derive_operational_result(tmp_path, proof, result, artifacts, binding)
        == module.PASSED_ACTION_COUNTS
    )


@pytest.mark.parametrize(
    "field,value",
    (
        ("kind", "pytest"),
        ("schema_version", True),
        ("contract", "caller-written.json"),
        ("events", "missing.json"),
    ),
)
def test_result_cannot_substitute_its_mode_or_registry_contract(evidence, field, value):
    root, proof, binding, result, artifacts = evidence
    result[field] = value
    with pytest.raises(ValueError):
        module.derive_operational_result(root, proof, result, artifacts, binding)


@pytest.mark.parametrize(
    "change",
    (
        "missing-post",
        "phase-order",
        "exit-bool",
        "overlap",
        "wrong-target",
        "wrong-nonce",
        "raw-digest",
        "observer-gold",
        "different-source",
        "no-transition",
        "wrong-binary",
    ),
)
def test_partial_replayed_or_fabricated_success_is_refused(evidence, change):
    root, proof, binding, result, artifacts = evidence
    record_path = root / result["events"]
    record = json.loads(record_path.read_text())
    step = record["steps"][2]
    if change == "missing-post":
        record["steps"].pop()
    elif change == "phase-order":
        record["steps"].reverse()
    elif change == "overlap":
        step["started_at"] = record["steps"][0]["started_at"]
    elif change == "different-source":
        record["binding"] = copy.deepcopy(binding)
        record["binding"]["source"]["head"] = "c" * 40
    else:
        if change in {"exit-bool", "raw-digest"}:
            path = step["execution"]
            value = json.loads((root / path).read_text())
            if change == "exit-bool":
                value["command"]["exit_code"] = False
            else:
                value["raw"][0]["sha256"] = "0" * 64
        elif change == "observer-gold":
            path = step["request"]
            value = json.loads((root / path).read_text())
            value["expected"] = {"success": True}
        else:
            path = step["stdout"]
            value = json.loads((root / path).read_text())
            if change == "wrong-target":
                value["target"]["state_root"] = "/another/root"
            elif change == "wrong-nonce":
                value["run_id"] = "0" * 32
            elif change == "wrong-binary":
                value["observed"]["daemon_sha256"] = "0" * 64
            else:
                pre = json.loads((root / record["steps"][0]["stdout"]).read_text())
                value["observed"] = pre["observed"]
        (root / path).write_text(json.dumps(value))
        if path == step["stdout"]:
            execution_path = root / step["execution"]
            execution = json.loads(execution_path.read_text())
            execution["raw"][0].update(
                sha256="sha256:" + digest((root / path).read_bytes()),
                bytes=len((root / path).read_bytes()),
            )
            execution_path.write_text(json.dumps(execution))
        artifacts[:] = [archive(root, item["source_path"]) for item in artifacts]
    record_path.write_text(json.dumps(record))
    artifacts[:] = [archive(root, item["source_path"]) for item in artifacts]
    with pytest.raises(ValueError):
        module.derive_operational_result(root, proof, result, artifacts, binding)


@pytest.mark.parametrize(
    "key,value",
    (
        ("root_incarnation", "backup-incarnation"),
        ("root_incarnation", "live-before"),
        ("data_sha256", "0" * 64),
        ("sequence_high_water", 42),
        ("daemon_sha256", "0" * 64),
    ),
)
def test_restore_forward_preserves_backup_and_rotates_incarnation(tmp_path, key, value):
    path = install_contract(tmp_path, "p11-rollback")
    contract = json.loads((tmp_path / path).read_text())
    pre, post = fixture_states(contract)
    post[key] = value
    with pytest.raises(ValueError):
        module.validate_transition(contract, pre, post)


def test_observer_cannot_be_the_action_producer(evidence):
    root, proof, _, result, _ = evidence
    contract = json.loads((root / result["contract"]).read_text())
    contract["actors"]["pre"] = contract["actors"]["action"]
    with pytest.raises(ValueError, match="independent"):
        module.validate_contract(root, proof, json.dumps(contract).encode())


@pytest.mark.parametrize(
    "field,path",
    (
        ("binary_path", "/"),
        ("binary_path", "//opt/quanta/bin/searchd"),
        ("binary_path", "/opt/quanta/./bin/searchd"),
        ("binary_path", "/opt/quanta/bin/searchd/"),
        ("binary_path", "/var/lib/quanta/state/bin/searchd"),
        ("config_path", "/opt/quanta/bin"),
        ("state_root", "/etc/quanta"),
        ("state_root", "/opt/quanta/bin/searchd"),
    ),
)
def test_operational_target_refuses_broad_aliasing_or_overlapping_paths(
    evidence, field, path
):
    root, proof, _, result, _ = evidence
    contract = json.loads((root / result["contract"]).read_text())
    contract["target"][field] = path
    with pytest.raises(ValueError, match="target paths|paths must not overlap"):
        module.validate_contract(root, proof, json.dumps(contract).encode())


def test_overlapping_target_refuses_before_host_probe_or_actor(evidence, monkeypatch):
    root, proof, binding, result, _ = evidence
    contract_path = root / result["contract"]
    contract = json.loads(contract_path.read_text())
    contract["target"]["binary_path"] = contract["target"]["state_root"] + "/searchd"
    contract_path.write_text(json.dumps(contract))
    monkeypatch.setattr(
        module, "observed_host_identity", lambda: pytest.fail("host probed before validation")
    )
    monkeypatch.setattr(module, "execute", lambda *a, **k: pytest.fail("actor executed"))
    output = root / "raw-overlapping-target"
    with pytest.raises(ValueError, match="paths must not overlap"):
        module.run_action(root, proof, binding, output, paired_checkout=root / "pair")
    assert not output.exists()


def test_staged_action_refuses_before_output_or_process(evidence, monkeypatch):
    root, proof, binding, _, _ = evidence
    proof["authority_state"] = "staged"
    monkeypatch.setattr(module, "execute", lambda *a, **k: pytest.fail("staged action executed"))
    output = root / "new-output"
    with pytest.raises(ValueError, match="staged"):
        module.run_action(root, proof, binding, output, paired_checkout=root / "pair")
    assert not output.exists()


@pytest.mark.parametrize("proof_id", tuple(module.ACTIONS))
def test_actual_frontdoor_retains_staged_refusal(proof_id, monkeypatch, tmp_path, capsys):
    monkeypatch.delenv("QUANTA_PROOF_RAW_DIR", raising=False)
    monkeypatch.delenv("SEMANTICA_CHECKOUT", raising=False)
    assert cli.main([proof_id, "--output", str(tmp_path / "never")]) == 2
    assert "staged operational action" in capsys.readouterr().err
    assert not (tmp_path / "never").exists()


@pytest.mark.parametrize("change", ("no-transition", "actor-drift", "none"))
def test_real_process_orchestration_checks_independent_state(evidence, monkeypatch, change):
    root, proof, binding, _, _ = evidence
    contract = json.loads((root / proof["operational_contract"]).read_text())
    pre, post = fixture_states(contract)
    state = root / "observed-state.json"
    state.write_text(json.dumps(post if change == "no-transition" else pre))
    observer = (
        "import json,sys,pathlib\n"
        "request=json.loads(pathlib.Path(sys.argv[2]).read_text())\n"
        f"request['observed']=json.loads(pathlib.Path({str(state)!r}).read_text())\n"
        "print(json.dumps(request))\n"
    )
    action = (
        "import json,sys,pathlib\n"
        "request=json.loads(pathlib.Path(sys.argv[2]).read_text())\n"
        "request.pop('expected')\n"
        "request['observed']={'action':'deployment','completed':True}\n"
    )
    if change != "no-transition":
        action += f"pathlib.Path({str(state)!r}).write_text({json.dumps(post)!r})\n"
    if change == "actor-drift":
        action += f"pathlib.Path({str(root / contract['actors']['post'])!r}).write_text('# changed observer')\n"
    action += "print(json.dumps(request))\n"
    for phase in ("pre", "post"):
        (root / contract["actors"][phase]).write_text(observer)
    (root / contract["actors"]["action"]).write_text(action)
    monkeypatch.setattr(module, "observed_host_identity", lambda: binding["host_identity_digest"])
    monkeypatch.setattr(module, "_check_source_binding", lambda *a: None)
    output = root / "raw-actual-fixture"
    if change != "none":
        with pytest.raises(ValueError, match="transition|changed during"):
            module.run_action(root, proof, binding, output, paired_checkout=root / "pair")
        assert not (output / "events.json").exists()
    else:
        result, sources = module.run_action(
            root, proof, binding, output, paired_checkout=root / "pair"
        )
        artifacts = [archive(root, source) for source in sources]
        assert (
            module.derive_operational_result(root, proof, result, artifacts, binding)
            == module.PASSED_ACTION_COUNTS
        )


@pytest.mark.parametrize(
    "proof_id,change",
    [
        ("p11-deployment", "already-achieved"),
        ("p11-activation", "already-achieved"),
        *[
            (proof_id, key)
            for proof_id in ("p11-activation", "p11-rollback")
            for key in sorted(module.BASE_STATE)
        ],
    ],
)
def test_inadmissible_pre_state_never_invokes_action(tmp_path, monkeypatch, proof_id, change):
    path = install_contract(tmp_path, proof_id)
    contract = json.loads((tmp_path / path).read_text())
    binding = fixture_binding(contract)
    proof = {
        "id": proof_id,
        "execution_mode": "operational-action",
        "authority_state": "executable",
        "operational_contract": path,
    }
    pre, post = fixture_states(contract)
    if change == "already-achieved":
        pre = post
    else:
        pre[change] = "0" * 64 if change.endswith("sha256") else "wrong-format"
    state = tmp_path / "live-state.json"
    state.write_text(json.dumps(pre))
    marker = tmp_path / "action-executed"
    observer = (
        "import json,sys,pathlib\n"
        "request=json.loads(pathlib.Path(sys.argv[2]).read_text())\n"
        f"request['observed']=json.loads(pathlib.Path({str(state)!r}).read_text())\n"
        "print(json.dumps(request))\n"
    )
    for phase in ("pre", "post"):
        (tmp_path / contract["actors"][phase]).write_text(observer)
    (tmp_path / contract["actors"]["action"]).write_text(
        f"import pathlib\npathlib.Path({str(marker)!r}).touch()\n"
    )
    monkeypatch.setattr(module, "observed_host_identity", lambda: binding["host_identity_digest"])
    monkeypatch.setattr(module, "_check_source_binding", lambda *a: None)
    output = tmp_path / "raw-inadmissible-pre"
    with pytest.raises(ValueError, match="transition|pre-state"):
        module.run_action(tmp_path, proof, binding, output, paired_checkout=tmp_path / "pair")
    assert not marker.exists()
    assert not (output / "action").exists()
    assert not (output / "events.json").exists()


@pytest.mark.parametrize("field", ("state_root", "binary_path", "config_path", "config_sha256"))
def test_frontdoor_refuses_prerequisite_target_drift_before_any_actor(
    tmp_path, monkeypatch, capsys, field
):
    path = install_contract(tmp_path, "p11-activation")
    contract = json.loads((tmp_path / path).read_text())
    proof = {
        "id": "p11-activation",
        "authority_state": "executable",
        "artifact": "artifacts/activation.json",
        "artifact_schema": "schema.json",
        "paired_repository": "pair",
        "paired_dependency_lock": "Cargo.lock",
        "operational_contract": path,
    }
    predecessor = {"id": "p11-deployment"}
    target = module.target_identity(contract)
    target[field] = "different-target"
    payload = {
        "proof_id": predecessor["id"],
        "daemon_binary": {"path": "binary", "sha256": "1" * 64},
        "environment": {
            "toolchain": "fixture",
            "features": [],
            "host": {"identity_digest": contract["target"]["host_identity_digest"]},
        },
        "execution_result": {"kind": "operational", "target": target},
    }
    (tmp_path / "schema.json").write_text("{}")
    checker, writer = module._checker(), module._writer()
    monkeypatch.setattr(cli, "ROOT", tmp_path)
    monkeypatch.setattr(checker, "_read_toml", lambda *a: {"proofs": [proof, predecessor]})
    monkeypatch.setattr(checker, "check_registry", lambda *a, **k: [])
    monkeypatch.setattr(checker, "proof_source_snapshot", lambda *a, **k: {})
    monkeypatch.setattr(checker, "paired_source_snapshot", lambda *a, **k: {})
    monkeypatch.setattr(checker, "_payload_json", lambda *a, **k: payload)
    monkeypatch.setattr(checker, "check_manifest", lambda *a, **k: [])
    monkeypatch.setattr(module, "observed_host_environment", lambda: {})
    monkeypatch.setattr(writer, "_host_environment", lambda _: payload["environment"])
    monkeypatch.setattr(
        writer,
        "_resolve_dependencies",
        lambda *a: [{"proof_id": predecessor["id"], "path": "prior.json"}],
    )
    monkeypatch.setattr(module, "run_action", lambda *a, **k: pytest.fail("action invoked"))
    output = tmp_path / "never"
    assert (
        cli.main(
            [proof["id"], "--output", str(output), "--paired-checkout", str(tmp_path / "pair")]
        )
        == 2
    )
    assert "different target or configuration" in capsys.readouterr().err
    assert not output.exists()


def test_runner_rechecks_prerequisite_target_before_output(evidence, monkeypatch):
    root, proof, binding, result, _ = evidence
    proof.update(artifact_schema="schema.json", paired_repository="pair")
    target = copy.deepcopy(result["target"])
    target["state_root"] = "/different/live/state"
    payload = {
        "proof_id": "p11-activation",
        "execution_result": {"kind": "operational", "target": target},
    }
    (root / "prerequisite.json").write_text(json.dumps(payload))
    receipt = archive(root, "prerequisite.json")
    receipt["proof_id"] = payload["proof_id"]
    binding["dependency_receipts"] = [receipt]
    checker = module._checker()
    monkeypatch.setattr(checker, "_read_toml", lambda *a: {"proofs": [{"id": payload["proof_id"]}]})
    monkeypatch.setattr(checker, "_payload_json", lambda *a, **k: {})
    monkeypatch.setattr(checker, "check_manifest", lambda *a, **k: [])
    monkeypatch.setattr(module, "observed_host_identity", lambda: binding["host_identity_digest"])
    monkeypatch.setattr(module, "_check_source_binding", lambda *a: None)
    monkeypatch.setattr(module, "execute", lambda *a, **k: pytest.fail("actor invoked"))
    output = root / "never"
    with pytest.raises(ValueError, match="different target or configuration"):
        module.run_action(root, proof, binding, output, paired_checkout=root / "pair")
    assert not output.exists()


def test_manifest_window_must_cover_every_observation(evidence):
    root, proof, binding, result, artifacts = evidence
    with pytest.raises(ValueError, match="execution window"):
        module.derive_operational_result(
            root,
            proof,
            result,
            artifacts,
            binding,
            window=("2026-09-21T00:00:00+00:00", "2026-09-21T00:00:01+00:00"),
        )


def test_target_summary_cannot_relabel_the_archived_contract(evidence):
    root, proof, binding, result, artifacts = evidence
    result["target"]["state_root"] = "/unrelated/state"
    with pytest.raises(ValueError, match="target summary"):
        module.derive_operational_result(root, proof, result, artifacts, binding)


@pytest.mark.parametrize("field", ("state_root", "binary_path", "config_sha256"))
def test_aggregate_requires_one_target_and_configuration(field):
    checker = module._checker()
    root = Path(module.__file__).resolve().parents[2]
    registry = checker._read_toml(root / "tools/ci/proof-authority.toml")
    proofs = {item["id"]: item for item in registry["proofs"]}
    source = {"head": "a" * 40, "dirty_digest": "sha256:" + "0" * 64}
    pair = {
        "repository": "github:josongmin/semantica-codegraph-v2",
        "remote_identity_digest": "sha256:" + "1" * 64,
        "source": source,
        "dependency_lock": {"path": "packages/analysis/quanta-v2/Cargo.lock", "sha256": "2" * 64},
    }
    payload = {
        "source": source,
        "source_pair": pair,
        "daemon_binary": {"path": "artifacts/binary", "sha256": "3" * 64},
        "environment": {
            "host": {"profile": "linux-production-like", "identity_digest": "sha256:" + "4" * 64}
        },
        "state_root_format": "v2",
        "execution_result": {
            "kind": "operational",
            "target": {
                "host_identity_digest": "sha256:" + "4" * 64,
                "binary_path": "/opt/quanta/searchd",
                "config_path": "/etc/quanta/config",
                "state_root": "/var/lib/quanta/state",
                "config_sha256": "5" * 64,
            },
        },
    }
    payloads = {proof_id: copy.deepcopy(payload) for proof_id in module.ACTIONS}
    assert checker.check_aggregate(payloads, proof_by_id=proofs, path=root) == []
    payloads["p11-activation"]["execution_result"]["target"][field] = "different"
    findings = checker.check_aggregate(payloads, proof_by_id=proofs, path=root)
    assert any("one target and configuration" in item.message for item in findings)


def test_p11_action_cannot_be_promoted_as_a_test_proof():
    checker = module._checker()
    root = Path(module.__file__).resolve().parents[2]
    registry = checker._read_toml(root / "tools/ci/proof-authority.toml")
    proof = next(item for item in registry["proofs"] if item["id"] == "p11-deployment")
    proof["execution_mode"] = "test-authority"
    findings = checker.check_registry(
        registry, root=root, path=root / "tools/ci/proof-authority.toml"
    )
    assert any(
        "P11 actions require operational-action mode" in finding.message for finding in findings
    )


def test_archiving_actor_source_never_hides_dirty_code(tmp_path, monkeypatch):
    from tools.ci.tests.test_write_proof_manifest import _build_fixture_root, _run

    root, registry = _build_fixture_root(tmp_path)
    binary = root / "bin/searchd"
    binary.parent.mkdir()
    binary.write_bytes(b"release-daemon")
    contract_path = install_contract(root, "p11-deployment")
    _run(root, "git", "add", ".")
    _run(root, "git", "commit", "-qm", "independent operational fixture sources")
    proof = next(item for item in registry["proofs"] if item["id"] == "p11-deployment")
    proof.update(authority_state="executable", operational_contract=contract_path)
    proof.pop("staged_reason")
    contract = json.loads((root / contract_path).read_text())
    actor = root / contract["actors"]["action"]
    actor.write_text(actor.read_text() + "# uncommitted operational change\n")
    writer, checker = module._writer(), module._checker()
    environment = {
        "toolchain": "fixture",
        "features": [],
        "os": "linux",
        "arch": "fixture",
        "host": {
            "profile": "linux-production-like",
            "cpu_count": 1,
            "memory_bytes": 1,
            "identity_digest": "sha256:" + "3" * 64,
        },
    }
    monkeypatch.setattr(writer, "_host_environment", lambda value: environment)
    monkeypatch.setattr(
        writer, "_resolve_source_pair", lambda *a, **k: {"source": {"head": "b" * 40}}
    )
    monkeypatch.setattr(writer, "_resolve_dependencies", lambda *a, **k: [])
    terminal = {
        "status": "passed",
        "counts": module.PASSED_ACTION_COUNTS,
        "environment": environment,
        "daemon_binary": "bin/searchd",
        "state_root_format": "v2",
        "inputs": dict.fromkeys(("fixture", "corpus", "config", "model", "provider")),
        "started_at": "2026-09-21T00:00:00+00:00",
        "ended_at": "2026-09-21T00:00:06+00:00",
        "artifacts": [contract_path, *contract["actors"].values()],
    }
    payload, _ = writer.build_manifest(
        root=root,
        proof=proof,
        proof_by_id={proof["id"]: proof},
        terminal=terminal,
        checker=checker,
        paired_checkout=None,
    )
    assert payload["source"]["dirty_digest"] != checker.CLEAN_DIRTY_DIGEST


def test_p00_collection_and_execution_select_the_same_owner_files():
    root = Path(module.__file__).resolve().parents[2]
    recipe = (root / "Justfile").read_text().split("proof-p00-authority-freeze:", 1)[1]
    recipe = recipe.split("\nproof-p12a-proof-infrastructure:", 1)[0]
    lines = recipe.splitlines()
    collection = next(line for line in lines if "collect-pytest" in line)
    pattern = r"tools/ci/tests/test_[a-z0-9_]+\.py"
    collected = set(re.findall(pattern, collection))
    executed = set(re.findall(pattern, "\n".join(line for line in lines if line != collection)))
    assert collected == executed
    assert "tools/ci/tests/test_proof_operational_result.py" in collected
