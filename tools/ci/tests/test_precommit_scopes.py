"""Keep local changed-file lint selection aligned with each linter's inputs."""

import os
import re
import subprocess
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
CONFIG = ROOT / ".pre-commit-config.yaml"
WORKFLOW = ROOT / ".github/workflows/ci.yml"
CORRECTNESS_WORKFLOW = ROOT / ".github/workflows/correctness.yml"


def test_scoped_repository_lints_skip_unrelated_docs_and_cover_their_inputs() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {
        hook["id"]: hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
    }
    inputs = {
        "lock-freshness": ("Cargo.lock", "scripts/check-lock-freshness.sh"),
        "workspace-lints": ("Cargo.toml", "scripts/check_workspace_lints.py"),
        "hexagonal-boundaries": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/lint-hexagonal-boundaries.py",
        ),
        "rust-no-allow": (
            "crates/quanta-index-core/src/lib.rs",
            "scripts/check-rust-allow-attributes.sh",
        ),
        "rust-derive-allowlist": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-rust-derive-allowlist.py",
        ),
        "rust-cargo-toml-hygiene": (
            "crates/quanta-index-core/Cargo.toml",
            "tools/ci/lint/check-cargo-toml-hygiene.py",
        ),
        "rust-module-discipline": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-module-discipline.py",
        ),
        "rust-module-cycles": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-module-cycles.py",
        ),
        "rust-error-shape": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-error-shape.py",
        ),
        "rust-wire-inventory": (
            "crates/quanta-index-contract/src/ipc/split.rs",
            "tools/ci/proof-aggregate.schema.json",
            "tools/ci/tests/test_write_proof_aggregate.py",
        ),
        "rust-digest-fallibility": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-digest-fallibility.py",
        ),
        "semgrep": (
            "crates/quanta-index-core/src/lib.rs",
            "scripts/run-semgrep.sh",
            ".semgrepignore",
        ),
    }

    for hook_id, positive_paths in inputs.items():
        hook = hooks[hook_id]
        assert not hook.get("always_run", False), hook_id
        assert hook["pass_filenames"] is (hook_id == "semgrep"), hook_id
        pattern = re.compile(hook["files"])
        for path in positive_paths:
            assert pattern.search(path), (hook_id, path)
        assert not pattern.search("docs/analysis/unrelated.md"), hook_id


def test_wire_inventory_scope_includes_tool_format_dependencies() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hook = next(
        hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
        if hook["id"] == "rust-wire-inventory"
    )
    pattern = re.compile(hook["files"])
    for path in (
        "Cargo.toml",
        "tools/ci/inventory/wire-surface.toml",
        "tools/ci/proof-authority.toml",
        "tools/ci/proof-manifest.schema.json",
        "tools/ci/proof-aggregate.schema.json",
        "tools/ci/error-authority-inventory.schema.json",
        "tools/ci/verification-receipt.schema.json",
        "tools/ci/tests/test_check_proof_authority.py",
        "tools/ci/tests/test_write_proof_aggregate.py",
        "tools/ci/tests/test_write_error_authority_inventory.py",
    ):
        assert pattern.search(path), path


def test_semgrep_keeps_code_scope_but_expands_policy_changes(tmp_path: Path) -> None:
    fake_semgrep = tmp_path / "semgrep"
    fake_semgrep.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n', encoding="utf-8")
    fake_semgrep.chmod(0o755)
    env = os.environ.copy()
    env["PATH"] = f"{tmp_path}:{env['PATH']}"

    def scan_targets(*paths: str) -> list[str]:
        result = subprocess.run(
            ["bash", str(ROOT / "scripts/run-semgrep.sh"), *paths],
            cwd=ROOT,
            env=env,
            check=True,
            capture_output=True,
            text=True,
        )
        arguments = result.stdout.splitlines()
        assert arguments[:4] == [
            "--config",
            str(ROOT / "tools/ci/semgrep/rules.yml"),
            "--error",
            "--timeout",
        ]
        assert arguments[4] == "300"
        return arguments[5:]

    code_paths = [
        "crates/quanta-index-core/src/lib.rs",
        "tools/ci/tests/test_precommit_scopes.py",
    ]
    assert scan_targets(*code_paths) == code_paths
    assert scan_targets() == ["."]
    for policy_path in (
        ".pre-commit-config.yaml",
        ".semgrepignore",
        "pyproject.toml",
        "scripts/run-semgrep.sh",
        "tools/ci/semgrep/rules.yml",
    ):
        assert scan_targets(*code_paths, policy_path) == ["."]


def test_ci_precommit_skips_only_hooks_owned_by_dedicated_full_jobs() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    jobs = workflow["jobs"]
    precommit_steps = jobs["pre-commit"]["steps"]
    install = next(step for step in precommit_steps if step.get("name") == "Install pre-commit")
    assert install["run"] == "python -m pip install 'pre-commit>=4.0.0'"
    run = next(step for step in precommit_steps if step.get("name") == "Run pre-commit")
    skipped = set(run["env"]["SKIP"].split(","))
    owners = {
        "actionlint": ("policy", "bash scripts/run-actionlint.sh"),
        "shellcheck": ("policy", "bash scripts/run-shellcheck.sh"),
        "ruff": ("prompt-manager", "python -m ruff check ."),
        "ruff-format": ("prompt-manager", "python -m ruff format --check ."),
        "prompt-manager-lint": ("prompt-manager", "tools/prompt-manager/pm.py lint"),
        "cargo-fmt-check": ("rust-fmt", "./scripts/cargow fmt"),
        "cargo-check": ("rust-msrv", "cargo +1.92.0 test --workspace --all-targets"),
        "hexagonal-boundaries": ("rust-policy", "lint-hexagonal-boundaries.py"),
        "rust-derive-allowlist": ("rust-policy", "check-rust-derive-allowlist.py"),
        "rust-cargo-toml-hygiene": ("rust-policy", "check-cargo-toml-hygiene.py"),
        "proof-authority": ("proof-authority-current-gate", "just proof-authority-lint"),
        "rust-digest-fallibility": ("rust-policy", "check-digest-fallibility.py"),
    }
    assert skipped == set(owners)
    for hook_id, (job_id, command) in owners.items():
        job_commands = "\n".join(str(step.get("run", "")) for step in jobs[job_id]["steps"])
        assert command in job_commands, (hook_id, job_id, command)


def test_sourcegraph_parity_generates_and_checks_in_one_pass() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    command = "python3 tools/benchmark/sourcegraph_parity.py --check --write"
    assert workflow.count("tools/benchmark/sourcegraph_parity.py") == 1
    assert command in workflow
    recipe = justfile.split("rust-bench-dsl-parity:\n", 1)[1].split("\n\n", 1)[0]
    assert recipe.count("tools/benchmark/sourcegraph_parity.py") == 1
    assert command in recipe


def test_prompt_manager_ci_lints_without_syncing_first() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    commands = "\n".join(
        str(step.get("run", "")) for step in workflow["jobs"]["prompt-manager"]["steps"]
    )
    assert "tools/prompt-manager/pm.py lint" in commands
    assert "tools/prompt-manager/pm.py sync" not in commands
    assert "git diff --exit-code" in commands


def test_proof_authority_ci_has_one_static_owner_and_one_test_owner() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    jobs = workflow["jobs"]
    rust_policy_commands = "\n".join(
        str(step.get("run", "")) for step in jobs["rust-policy"]["steps"]
    )
    assert "check-proof-authority.py" not in rust_policy_commands
    assert "pytest" not in rust_policy_commands
    p00_commands = "\n".join(
        str(step.get("run", "")) for step in jobs["proof-authority-current-gate"]["steps"]
    )
    assert "just proof-authority-lint" in p00_commands
    prompt_commands = "\n".join(
        str(step.get("run", "")) for step in jobs["prompt-manager"]["steps"]
    )
    assert "python -m pytest tools" in prompt_commands


def test_ci_python_jobs_install_only_their_runtime_imports() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    jobs = workflow["jobs"]
    expected = {
        "rust-policy": "python -m pip install 'jsonschema>=4.23.0' 'pyyaml>=6.0.2'",
        "proof-authority-current-gate": "python -m pip install 'jsonschema>=4.23.0'",
        "agent-output": "python -m pip install 'jsonschema>=4.23.0'",
    }
    for job_id, command in expected.items():
        installs = [
            step["run"]
            for step in jobs[job_id]["steps"]
            if str(step.get("name", "")).startswith("Install ") and "run" in step
        ]
        assert command in installs, (job_id, installs)
        assert all("-e ." not in install for install in installs), (job_id, installs)


def test_msrv_uses_one_all_target_compile_graph() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    commands = [
        str(step["run"])
        for step in workflow["jobs"]["rust-msrv"]["steps"]
        if "run" in step and "cargo +1.92.0" in str(step["run"])
    ]
    expected = "cargo +1.92.0 test --workspace --all-targets --all-features --locked --no-run"
    assert commands == [expected]

    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    recipe = justfile.split("rust-msrv:\n", 1)[1].split("\n\n", 1)[0]
    assert recipe.count("cargo +1.92.0") == 1
    assert expected in recipe


def test_exhaustive_correctness_jobs_do_not_duplicate_every_pr_build() -> None:
    workflow = yaml.safe_load(CORRECTNESS_WORKFLOW.read_text(encoding="utf-8"))
    jobs = workflow["jobs"]
    nightly_or_manual = (
        "github.event_name == 'schedule' || (github.event_name == 'workflow_dispatch' && "
        "inputs.proof_bundle_run_id == '')"
    )
    exhaustive = {
        "rust-miri",
        "rust-careful",
        "rust-tsan",
        "rust-asan",
        "rust-mutants",
        "rust-udeps",
        "rust-fuzz-smoke",
    }
    assert {
        job_id for job_id in exhaustive if jobs[job_id].get("if") == nightly_or_manual
    } == exhaustive

    # These gates are source-specific and remain on PRs, schedules and ordinary
    # manual runs, but a release-proof dispatch starts only its dedicated gate.
    structural_condition = (
        "github.event_name != 'workflow_dispatch' || inputs.proof_bundle_run_id == ''"
    )
    for job_id in (
        "rust-llvm-lines",
        "rust-public-api",
        "rust-cargo-modules",
    ):
        assert jobs[job_id].get("if") == structural_condition, job_id
    assert jobs["rust-changed-line-coverage"].get("if") == "github.event_name == 'pull_request'"

    assert jobs["proof-authority-release-gate"]["if"] == (
        "github.event_name == 'workflow_dispatch' && inputs.proof_bundle_run_id != ''"
    )
    assert jobs["dsl-bench-latency"]["if"] == nightly_or_manual
