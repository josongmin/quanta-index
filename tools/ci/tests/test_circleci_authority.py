"""CircleCI rails must remain reachable and propagate command failures."""

from __future__ import annotations

import importlib.util
import os
import runpy
import subprocess
from pathlib import Path

import pytest
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
                "when": yaml.safe_load(CONFIG.read_text(encoding="utf-8"))["workflows"]["regular"][
                    "when"
                ],
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
    assert not _checker()._circleci_gate_enabled(
        config["workflows"]["regular"], run_heavy=True, event="pull_request", branch="main"
    )
    assert config["workflows"]["manual-heavy"]["when"] == "<< pipeline.parameters.run_heavy >>"


@pytest.mark.parametrize(
    ("event", "branch", "expected_tier"),
    [
        ("pull_request", "feature", "pr"),
        ("pull_request", "main", "pr"),
        ("push", "main", "main"),
        ("push", "feature", None),
        ("push", "", None),
        ("api", "main", None),
        ("tag", "main", None),
        ("", "main", None),
    ],
)
def test_regular_admission_matches_receipt_context_before_cargo(event, branch, expected_tier):
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    regular = config["workflows"]["regular"]
    module = _checker()
    assert module._circleci_gate_enabled(regular, run_heavy=False, event=event, branch=branch) is (
        expected_tier is not None
    )
    assert not module._circleci_gate_enabled(regular, run_heavy=True, event=event, branch=branch)
    steps = config["jobs"]["verify-rust-tests"]["steps"]
    script = next(
        step["run"]["command"]
        for step in steps
        if "run" in step and step["run"]["name"] == "rust nextest"
    )
    preflight = steps[1]["run"]
    assert preflight["name"] == "Verify verification receipt context"
    selection = 'if [[ "$CI_EVENT_NAME"' + script.split('if [[ "$CI_EVENT_NAME"', 1)[1]
    selection = selection.split("fi\n", 1)[0] + "fi\n"
    for context_script in (preflight["command"], "set -euo pipefail\n" + selection):
        result = subprocess.run(
            ["bash", "-c", context_script + 'printf "%s\\n%s\\n" "$tier" "$rail"\n'],
            env={**os.environ, "CI_EVENT_NAME": event, "CIRCLE_BRANCH": branch},
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
        if expected_tier is None:
            assert result.returncode == 2
            assert "No PR/main verification receipt" in result.stderr
            assert result.stdout == ""
        else:
            assert result.returncode == 0, result.stderr
            assert result.stdout.splitlines() == [
                expected_tier,
                f"{expected_tier}-workspace-nextest",
            ]


def test_regular_gate_rejects_unresolved_or_unsupported_logic():
    module = _checker()
    for gate in (
        {"when": {"matches": []}},
        {"when": "<< pipeline.unknown >>"},
        {"when": True, "unless": False},
        {"when": {"and": []}},
    ):
        with pytest.raises(ValueError):
            module._circleci_gate_enabled(gate, run_heavy=False, event="push", branch="main")


def test_authority_rejects_old_feature_push_admission(tmp_path: Path):
    data, path = _config(tmp_path)
    data["workflows"]["regular"].pop("when")
    data["workflows"]["regular"]["unless"] = "<< pipeline.parameters.run_heavy >>"
    path.write_text(yaml.safe_dump(data), encoding="utf-8")
    assert any("receipt context mismatch" in item for item in _rail_violations(tmp_path))


def test_all_job_commands_propagate_failures():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    for job in config["jobs"].values():
        for step in job["steps"]:
            run = step.get("run") if isinstance(step, dict) else None
            if run is not None:
                assert run["command"].lstrip().startswith("set -euo pipefail\n")


def test_regular_python_and_rust_jobs_are_independent_and_source_bound():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    workers = ["verify-rust-static", "verify-rust-tests", "verify-rust-docs", "verify-rust-bench"]
    entries = config["workflows"]["regular"]["jobs"]
    assert entries[:2] == [{"verify": {"requires": workers}}, "verify-python"]
    assert entries[2:-1] == workers
    assert entries[-1] == {
        "verify-pr-coverage": {
            "requires": ["verify"],
            "filters": 'pipeline.event.name == "pull_request"',
        }
    }
    gate_steps = config["jobs"]["verify"]["steps"]
    assert gate_steps[0] == "checkout"
    assert 'test "$(git rev-parse HEAD)" = "$CIRCLE_SHA1"' in gate_steps[1]["run"]["command"]
    for name in workers:
        steps = config["jobs"][name]["steps"]
        common_steps = steps[:1] + steps[2:] if name == "verify-rust-tests" else steps
        assert common_steps[:3] == config["jobs"]["verify-python"]["steps"][:3]
        source_guard = steps[-1]["run"]["command"]
        assert 'test "$(git rev-parse HEAD)" = "$CIRCLE_SHA1"' in source_guard
        assert "git diff --exit-code" in source_guard
        assert "git status --porcelain=v1 --untracked-files=all" in source_guard
        assert "exit 1" in source_guard
    rust_steps = config["jobs"]["verify-rust-tests"]["steps"]
    python_steps = config["jobs"]["verify-python"]["steps"]
    assert rust_steps[0] == python_steps[0] == "checkout"
    assert rust_steps[2:4] == python_steps[1:3]
    assert '"$HOME/.zprofile"' in rust_steps[2]["run"]["command"]
    assert 'test "$(git rev-parse HEAD)" = "$CIRCLE_SHA1"' in rust_steps[3]["run"]["command"]
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
    assert (
        "uv run --frozen --extra dev python tools/ci/lint/check-cargo-modules-snapshot.py"
        in module_step["command"]
    )
    python_install = next(
        step["run"]
        for step in python_steps
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Install locked Python dependencies"
    )
    assert '"$HOME/.zprofile"' in python_install["command"]
    assert "zsh -lc 'python3 --version'" in python_install["command"]
    assert "Python policy and tooling tests" in python_names
    assert "Produce and validate P00 authority manifest" in python_names
    assert "Python policy and tooling tests" not in rust_names


def test_regular_ci_runs_the_pinned_public_api_ratchet():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    regular_jobs = config["workflows"]["regular"]["jobs"]
    assert "verify-python" in regular_jobs
    steps = config["jobs"]["verify-python"]["steps"]
    gate_index = next(
        index
        for index, step in enumerate(steps)
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Verify guarded public API snapshots"
    )
    gate = steps[gate_index]["run"]
    python_install_index = next(
        index
        for index, step in enumerate(steps)
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Install locked Python dependencies"
    )
    tool_install_index = next(
        index
        for index, step in enumerate(steps)
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Install pinned public API tooling"
    )
    assert python_install_index < tool_install_index < gate_index
    assert gate_index < next(
        index
        for index, step in enumerate(steps)
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Python policy and tooling tests"
    )
    assert gate.get("when", "always") == "always"
    command = gate["command"]
    assert command.startswith("set -euo pipefail\nunset BASH_ENV ENV\n")
    assert "--update-baseline" not in command
    public_api = runpy.run_path(str(ROOT / "tools/ci/lint/check-public-api.py"))
    assert public_api["GUARDED_CRATES"] == ["quanta-index-contract", "quanta-index-sdk"]
    toolchain = public_api["PUBLIC_API_TOOLCHAIN"]
    install = steps[tool_install_index]["run"]["command"]
    assert install.startswith("set -euo pipefail\nunset BASH_ENV ENV\n")
    assert f"rustup toolchain install {toolchain} --profile minimal" in install
    assert "cargo install cargo-public-api --version 0.51.0 --locked" in install
    assert '[[ "$(cargo public-api --version)" == "cargo-public-api 0.51.0" ]]' in install
    assert (
        "uv run --frozen --extra dev python tools/ci/lint/check-public-api.py"
        in command.splitlines()
    )
    assert steps[tool_install_index - 1].get("restore_cache")
    cache = steps[tool_install_index - 1]["restore_cache"]["keys"][0]
    assert "cargo-public-api0.51.0" in cache
    assert steps[gate_index - 1]["save_cache"]["paths"] == ["~/.cargo/bin/cargo-public-api"]


def test_pr_coverage_uses_exact_base_and_fails_closed():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    steps = config["jobs"]["verify-pr-coverage"]["steps"]
    coverage = next(
        step["run"]
        for step in steps
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Changed production Rust line coverage"
    )
    assert coverage["environment"]["CI_PR_BASE_SHA"] == (
        "<< pipeline.event.github.pull_request.base.sha >>"
    )
    command = coverage["command"]
    assert "git cat-file -e" in command
    assert "--minimum-percent 90" in command
    assert "llvm-cov nextest" in command
    assert "cargo install cargo-llvm-cov --version 0.8.5 --locked" in next(
        step["run"]["command"]
        for step in steps
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Install pinned coverage tooling"
    )


def test_bench_capacity_matches_worker_without_changing_compile_scope():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    bench = config["jobs"]["verify-rust-bench"]
    assert bench["resource_class"] == "large"
    assert bench["environment"]["CARGO_BUILD_JOBS"] == "4"
    command = next(
        step["run"]["command"]
        for step in bench["steps"]
        if isinstance(step, dict)
        and step.get("run", {}).get("name") == "Rust benchmark compilation"
    )
    assert command.splitlines()[-1] == "just rust-bench-build"
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    recipe = justfile.split("rust-bench-build:\n", 1)[1].split("\n\n", 1)[0]
    assert recipe.strip() == (
        "{{cargo}} --lane bench-lane bench --workspace --all-features --locked --no-run"
    )
    for name in ("verify-rust-static", "verify-rust-tests", "verify-rust-docs"):
        assert config["jobs"][name]["resource_class"] == "medium"
        assert config["jobs"][name]["environment"]["CARGO_BUILD_JOBS"] == "2"


def test_regular_workflow_keeps_standard_rust_and_precommit_gates():
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    rust_workers = (
        "verify-rust-static",
        "verify-rust-tests",
        "verify-rust-docs",
        "verify-rust-bench",
    )
    python_steps = config["jobs"]["verify-python"]["steps"]
    rust_runs = {
        step["run"]["name"]: step["run"]["command"]
        for name in rust_workers
        for step in config["jobs"][name]["steps"]
        if isinstance(step, dict) and "run" in step
    }
    python_runs = {
        step["run"]["name"]: step["run"]["command"] for step in python_steps if "run" in step
    }
    for name, commands in {
        "Rust dependency and MSRV checks": (
            "just rust-machete",
            "just rust-msrv",
        ),
        "Rust documentation": ("just rust-doc",),
        "Rust benchmark compilation": ("just rust-bench-build",),
    }.items():
        script = rust_runs[name]
        assert script.startswith("set -euo pipefail\n")
        assert "unset BASH_ENV ENV\n" in script
        assert all(f"\n{command}\n" in script for command in commands)
    precommit = python_runs["Pre-commit hooks"]
    assert "just rust-policy" in python_runs["Python policy and tooling tests"]
    assert precommit.startswith("set -euo pipefail\n")
    assert (
        "uv run --frozen --extra dev pre-commit run --all-files --show-diff-on-failure" in precommit
    )
    assert "git diff --exit-code" in precommit


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
