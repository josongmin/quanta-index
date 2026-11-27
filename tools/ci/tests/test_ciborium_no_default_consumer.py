"""Keep the registry no-default proof tied to Cargo's resolved feature graph."""

from __future__ import annotations

import subprocess
from copy import deepcopy

import pytest
import yaml

from tools.ci import verify_ciborium_no_default as verifier


def _metadata():
    packages = [
        {
            "id": name,
            "name": name,
            "version": "0.2.2" if name == "ciborium" else "1.0.0",
            "source": verifier.REGISTRY,
        }
        for name in sorted(verifier.RUNTIME_PACKAGES)
    ]
    packages.append(
        {
            "id": "root",
            "name": "ciborium-no-default-consumer",
            "source": None,
            "dependencies": [{"name": "ciborium", "uses_default_features": False}],
        }
    )
    return {
        "packages": packages,
        "resolve": {"nodes": [{"id": package["id"], "features": []} for package in packages]},
    }


def test_accepts_single_registry_runtime_without_std():
    verifier.validate(_metadata())


def test_rejects_std_enabled_by_transitive_feature():
    metadata = _metadata()
    node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == "serde_core")
    node["features"].append("std")
    with pytest.raises(ValueError, match="serde_core unexpectedly enables std"):
        verifier.validate(metadata)


def test_rejects_local_or_duplicate_supplier():
    metadata = _metadata()
    package = next(package for package in metadata["packages"] if package["name"] == "ciborium")
    package["source"] = None
    with pytest.raises(ValueError, match="ciborium must resolve once from crates.io"):
        verifier.validate(metadata)
    package["source"] = verifier.REGISTRY
    metadata["packages"].append(deepcopy(package))
    with pytest.raises(ValueError, match="ciborium must resolve once from crates.io"):
        verifier.validate(metadata)


def test_rejects_default_features_and_missing_resolved_nodes():
    metadata = _metadata()
    root = next(package for package in metadata["packages"] if package["id"] == "root")
    root["dependencies"][0]["uses_default_features"] = True
    with pytest.raises(ValueError, match="consumer must disable"):
        verifier.validate(metadata)
    root["dependencies"][0]["uses_default_features"] = False
    metadata["resolve"]["nodes"] = []
    with pytest.raises(ValueError, match="lacks a resolved node"):
        verifier.validate(metadata)


def test_ci_runs_nonempty_registry_consumer_and_checks_its_features():
    config = yaml.safe_load((verifier.ROOT / ".circleci/config.yml").read_text())
    steps = config["jobs"]["verify-rust-tests"]["steps"]
    commands = {
        step["run"]["name"]: step["run"]["command"]
        for step in steps
        if isinstance(step, dict)
        and isinstance(step.get("run"), dict)
        and "name" in step["run"]
        and "command" in step["run"]
    }
    command = commands["registry ciborium no-default consumer"]
    assert "tools/ci/ciborium-no-default-consumer/Cargo.toml" in command
    assert "nextest run" in command and "--locked" in command and "--no-tests fail" in command
    assert "python3 tools/ci/verify_ciborium_no_default.py" in command


def test_local_commands_run_the_same_registry_consumer_and_check_its_features():
    commands = []
    for recipe in ("rust-test-ciborium-no-default", "rust-test-ciborium-registry-no-default"):
        result = subprocess.run(
            ["just", "--dry-run", recipe],
            cwd=verifier.ROOT,
            capture_output=True,
            text=True,
        )
        assert result.returncode == 0, result.stderr
        commands.append(result.stderr.splitlines())

    assert commands[0] == commands[1]
    run, verify = commands[0]
    assert "nextest run --manifest-path tools/ci/ciborium-no-default-consumer/Cargo.toml" in run
    assert "--locked --lib --no-tests fail" in run
    assert verify == "python3 tools/ci/verify_ciborium_no_default.py"
