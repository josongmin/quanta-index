"""CircleCI rails must remain reachable and propagate command failures."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
CHECKER = ROOT / "tools/ci/lint/check-test-authority.py"
CONFIG = ROOT / ".circleci/config.yml"


def _checker():
    spec = importlib.util.spec_from_file_location("check_test_authority_circleci", CHECKER)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    import sys

    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _rail_violations(root: Path, command: str = "just owner-test"):
    module = _checker()
    violations = []
    module._validate_rail_binding(
        root=root,
        catalog=root / "authority.toml",
        rail_id="pr-owner",
        raw_rail={
            "workflow": ".circleci/config.yml",
            "job": "verify",
            "step": "owner test",
            "tier": "pr",
        },
        command=command,
        violations=violations,
    )
    return [violation.message for violation in violations]


def _config(root: Path):
    path = root / ".circleci/config.yml"
    path.parent.mkdir(parents=True)
    data = {
        "version": 2.1,
        "parameters": {"run_heavy": {"type": "boolean", "default": False}},
        "jobs": {
            "verify": {
                "steps": [
                    {
                        "run": {
                            "name": "owner test",
                            "command": "set -euo pipefail\njust owner-test\n",
                        }
                    }
                ]
            }
        },
        "workflows": {
            "regular": {
                "unless": "<< pipeline.parameters.run_heavy >>",
                "jobs": ["verify"],
            }
        },
    }
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    return data, path


def test_real_catalog_binds_to_circleci():
    assert _checker().audit_catalog() == []


def test_circleci_custody_unset_allows_only_shell_startup_hooks(tmp_path: Path):
    data, path = _config(tmp_path)
    command = data["jobs"]["verify"]["steps"][0]["run"]
    command["command"] = "set -euo pipefail\nunset BASH_ENV ENV\njust owner-test\n"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert _rail_violations(tmp_path) == []

    command["command"] = "set -euo pipefail\nunset PATH\njust owner-test\n"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any("does not execute declared command" in item for item in _rail_violations(tmp_path))


def test_circleci_rail_rejects_detached_and_failure_swallowing_steps(tmp_path: Path):
    data, path = _config(tmp_path)
    assert _rail_violations(tmp_path) == []

    data["workflows"]["regular"]["jobs"] = []
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any("not in CircleCI regular" in message for message in _rail_violations(tmp_path))

    data["workflows"]["regular"]["jobs"] = ["verify"]
    data["jobs"]["verify"]["steps"][0]["run"]["command"] = "set +e\njust owner-test\ntrue\n"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any(
        "does not execute declared command" in message for message in _rail_violations(tmp_path)
    )

    data["jobs"]["verify"]["steps"][0]["run"]["command"] = "set -euo pipefail\njust owner-test\n"
    data["jobs"]["verify"]["steps"][0]["run"]["when"] = "on_fail"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any("conditional" in message for message in _rail_violations(tmp_path))


def test_heavy_work_is_off_by_default():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    assert config["parameters"]["run_heavy"]["default"] is False
    assert config["workflows"]["regular"]["unless"] == "<< pipeline.parameters.run_heavy >>"
    assert config["workflows"]["manual-heavy"]["when"] == "<< pipeline.parameters.run_heavy >>"


def test_regular_python_and_rust_jobs_are_independent_and_source_bound():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    assert config["workflows"]["regular"]["jobs"] == ["verify", "verify-python"]
    rust_steps = config["jobs"]["verify"]["steps"]
    python_steps = config["jobs"]["verify-python"]["steps"]
    assert rust_steps[0] == python_steps[0] == "checkout"
    assert rust_steps[1:3] == python_steps[1:3]
    assert 'test "$(git rev-parse HEAD)" = "$CIRCLE_SHA1"' in rust_steps[2]["run"]["command"]
    rust_names = {step["run"]["name"] for step in rust_steps if "run" in step}
    python_names = {step["run"]["name"] for step in python_steps if "run" in step}
    assert "rust nextest" in rust_names and "rust nextest" not in python_names
    module_step = next(
        step["run"]
        for step in python_steps
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Verify guarded module snapshots"
    )
    assert "cargo install cargo-modules --version 0.26.0 --locked" in module_step["command"]
    assert "python3 tools/ci/lint/check-cargo-modules-snapshot.py" in module_step["command"]
    assert "Python policy and tooling tests" in python_names
    assert "P00 authority owner tests" in python_names
    assert "Python policy and tooling tests" not in rust_names


def test_circleci_explicit_test_selector_must_be_registered(tmp_path: Path):
    data, path = _config(tmp_path)
    data["jobs"]["verify"]["steps"][0]["run"]["command"] = (
        "./scripts/cargow test -p demo --test missing\n"
    )
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    module = _checker()
    violations = []
    module._validate_workflow_test_selectors(
        root=tmp_path,
        catalog=tmp_path / "authority.toml",
        targets={"demo-covered": {"owner": "demo", "target": "covered"}},
        violations=violations,
    )
    assert any(
        "selects unknown Cargo test target demo:missing" in item.message for item in violations
    )
