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


def test_ci_avoids_duplicate_branch_push_and_pull_request_runs() -> None:
    workflow = yaml.load(WORKFLOW.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)
    triggers = workflow["on"]
    assert triggers["push"] == {"branches": ["main"]}
    assert "pull_request" in triggers
    assert "merge_group" in triggers


def test_p00_proof_step_stops_before_manifest_when_recipe_fails(tmp_path: Path) -> None:
    workflow = yaml.load(WORKFLOW.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)
    steps = workflow["jobs"]["proof-authority-current-gate"]["steps"]
    script = next(
        step["run"]
        for step in steps
        if step.get("name") == "Produce P00 authority proof from the static gate"
    )
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    fake_just = fake_bin / "just"
    fake_just.write_text("#!/bin/sh\nexit 42\n", encoding="utf-8")
    fake_just.chmod(0o755)
    env = os.environ.copy()
    env["PATH"] = f"{fake_bin}:{env['PATH']}"

    result = subprocess.run(
        ["bash", "-c", script], cwd=tmp_path, env=env, capture_output=True, text=True
    )

    assert result.returncode == 42
    assert not (tmp_path / "artifacts/proof-authority/raw/p00-terminal-input.json").exists()


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
        "workspace-lints": (
            "Cargo.toml",
            "crates/quanta-index-core/Cargo.toml",
            "crates/quanta-index-core/src/lib.rs",
            "benchmarks/retrieval/Cargo.toml",
            "benchmarks/retrieval/src/lib.rs",
            "scripts/check_workspace_lints.py",
            "tools/ci/tests/test_check_workspace_lints.py",
        ),
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
            "crates/quanta-index-core/fuzz/fuzz_targets/silent.rs",
            "benchmarks/retrieval/src/lib.rs",
            ".github/workflows/ci.yml",
            "tools/ci/semgrep/rules.yml",
            "scripts/run-semgrep.sh",
            ".semgrepignore",
        ),
        "semgrep-rule-tests": (
            "tools/ci/semgrep/rules.yml",
            "tools/ci/tests/test_semgrep_policy.py",
            "scripts/check-semgrep-rules.sh",
        ),
        "precommit-scope-tests": (
            ".pre-commit-config.yaml",
            ".github/workflows/ci.yml",
            "Justfile",
            "tools/ci/tests/test_precommit_scopes.py",
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


def test_semgrep_counterexamples_have_one_test_owner() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {
        hook["id"]: hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
    }
    generic = re.compile(hooks["prompt-manager-tests"]["files"])
    dedicated = re.compile(hooks["semgrep-rule-tests"]["files"])
    for path in (
        "tools/ci/semgrep/rules.yml",
        "tools/ci/tests/test_semgrep_policy.py",
    ):
        assert not generic.search(path), path
        assert dedicated.search(path), path
    assert generic.search("scripts/run-tooling-tests.sh")
    assert not generic.search("tools/ci/tests/test_precommit_scopes.py")
    assert not generic.search(".pre-commit-config.yaml")
    scoped = re.compile(hooks["precommit-scope-tests"]["files"])
    assert scoped.search("tools/ci/tests/test_precommit_scopes.py")
    assert scoped.search(".pre-commit-config.yaml")
    assert "tools/ci/tests/test_precommit_scopes.py" in hooks["precommit-scope-tests"]["entry"]
    scanner = re.compile(hooks["semgrep"]["files"])
    for irrelevant in (
        "scripts/check-semgrep-rules.sh",
        "crates/quanta-index-core/tests/operation_journal.rs",
        "benchmarks/retrieval/tests/fixture.rs",
        "tools/ci/tests/test_semgrep_policy.py",
        "tools/ci/tests/test_precommit_scopes.py",
        "pyproject.toml",
        ".pre-commit-config.yaml",
    ):
        assert not scanner.search(irrelevant), irrelevant
    assert hooks["semgrep-rule-tests"]["entry"] == "bash scripts/check-semgrep-rules.sh"
    runner = (ROOT / "scripts/run-tooling-tests.sh").read_text(encoding="utf-8")
    assert "--ignore=tools/ci/tests/test_semgrep_policy.py" in runner
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    verify = justfile.split("verify:\n", 1)[1].split("\n\n", 1)[0]
    assert verify.count("@just semgrep-rule-tests") == 1
    assert verify.count("@just semgrep\n") == 1


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
        assert arguments[:8] == [
            "--config",
            str(ROOT / "tools/ci/semgrep/rules.yml"),
            "--error",
            "--strict",
            "--metrics",
            "off",
            "--disable-version-check",
            "--timeout",
        ]
        assert arguments[8] == "300"
        return arguments[9:]

    code_paths = [
        "crates/quanta-index-core/src/lib.rs",
        "tools/ci/tests/test_precommit_scopes.py",
    ]
    assert scan_targets(*code_paths) == code_paths
    assert scan_targets() == ["."]
    for policy_path in (
        ".semgrepignore",
        "scripts/run-semgrep.sh",
        "tools/ci/semgrep/rules.yml",
    ):
        assert scan_targets(*code_paths, policy_path) == ["."]
        assert scan_targets(*code_paths, f"./{policy_path}") == ["."]
        assert scan_targets(*code_paths, str(ROOT / policy_path)) == ["."]
    for unrelated in (".pre-commit-config.yaml", "pyproject.toml"):
        assert scan_targets(*code_paths, unrelated) == [*code_paths, unrelated]


def test_ci_precommit_skips_only_hooks_owned_by_dedicated_full_jobs() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    jobs = workflow["jobs"]
    precommit_steps = jobs["pre-commit"]["steps"]
    install = next(step for step in precommit_steps if step.get("name") == "Install pre-commit")
    assert install["run"] == "python -m pip install 'pre-commit>=4.0.0'"
    run = next(step for step in precommit_steps if step.get("name") == "Run pre-commit")
    skipped = set(run["env"]["SKIP"].split(","))
    owners = {
        "ruff": ("prompt-manager", "python -m ruff check ."),
        "ruff-format": ("prompt-manager", "python -m ruff format --check ."),
        "prompt-manager-lint": ("prompt-manager", "tools/prompt-manager/pm.py lint"),
        "cargo-fmt-check": ("rust-fmt", "./scripts/cargow fmt"),
        "cargo-check": (
            "rust-msrv",
            "RUSTUP_TOOLCHAIN=1.92.0 ./scripts/cargow --lane msrv-lane test --workspace",
        ),
        "hexagonal-boundaries": ("rust-policy", "lint-hexagonal-boundaries.py"),
        "rust-derive-allowlist": ("rust-policy", "check-rust-derive-allowlist.py"),
        "rust-cargo-toml-hygiene": ("rust-policy", "check-cargo-toml-hygiene.py"),
        "proof-authority": ("proof-authority-current-gate", "just proof-p00-authority-freeze"),
        "rust-digest-fallibility": ("rust-policy", "check-digest-fallibility.py"),
    }
    assert skipped == set(owners)
    for hook_id, (job_id, command) in owners.items():
        job_commands = "\n".join(str(step.get("run", "")) for step in jobs[job_id]["steps"])
        assert command in job_commands, (hook_id, job_id, command)
    assert {"actionlint", "shellcheck"}.isdisjoint(skipped)
    assert "policy" not in jobs


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
    steps = workflow["jobs"]["prompt-manager"]["steps"]
    commands = "\n".join(str(step.get("run", "")) for step in steps)
    assert "tools/prompt-manager/pm.py lint" in commands
    assert "tools/prompt-manager/pm.py sync" not in commands
    assert "git diff --exit-code" in commands
    install = next(
        step["run"] for step in steps if step.get("name") == "Install prompt-manager deps"
    )
    assert "-e ." not in install
    for dependency in ("jinja2", "jsonschema", "pyyaml", "pytest", "ruff"):
        assert dependency in install


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
    assert "just proof-p00-authority-freeze" in p00_commands
    assert "just proof-authority-current-gate" in p00_commands
    prompt_commands = "\n".join(
        str(step.get("run", "")) for step in jobs["prompt-manager"]["steps"]
    )
    assert "bash scripts/run-tooling-tests.sh" in prompt_commands
    semgrep_commands = "\n".join(str(step.get("run", "")) for step in jobs["semgrep"]["steps"])
    assert semgrep_commands.count("bash scripts/check-semgrep-rules.sh") == 1
    assert "bash scripts/run-semgrep.sh" in semgrep_commands


def test_ci_python_jobs_install_only_their_runtime_imports() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    jobs = workflow["jobs"]
    expected = {
        "rust-policy": "python -m pip install 'jsonschema>=4.23.0' 'pyyaml>=6.0.2' 'tree-sitter-language-pack==0.9.1'",
        "proof-authority-current-gate": "python -m pip install 'jsonschema>=4.23.0' 'pytest>=8.3.0'",
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

    correctness = yaml.safe_load(CORRECTNESS_WORKFLOW.read_text(encoding="utf-8"))
    release_steps = correctness["jobs"]["proof-authority-bundle-gate"]["steps"]
    release_install = next(
        step["run"]
        for step in release_steps
        if step.get("name") == "Install proof-authority dependencies"
    )
    assert release_install == "python -m pip install 'jsonschema>=4.23.0'"


def test_msrv_uses_one_all_target_compile_graph() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    commands = [
        str(step["run"])
        for step in workflow["jobs"]["rust-msrv"]["steps"]
        if "run" in step and "RUSTUP_TOOLCHAIN=1.92.0" in str(step["run"])
    ]
    expected = (
        "RUSTUP_TOOLCHAIN=1.92.0 ./scripts/cargow --lane msrv-lane test "
        "--workspace --all-targets --all-features --locked --no-run"
    )
    assert commands == [expected]
    assert all(
        "Remove rust-toolchain override" != step.get("name")
        for step in workflow["jobs"]["rust-msrv"]["steps"]
    )

    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    recipe = justfile.split("rust-msrv:\n", 1)[1].split("\n\n", 1)[0]
    assert recipe.count("RUSTUP_TOOLCHAIN=1.92.0") == 1
    assert "{{cargo}} --lane msrv-lane test --workspace --all-targets" in recipe
    assert "bash -lc" not in recipe


def test_exhaustive_correctness_jobs_do_not_duplicate_every_pr_build() -> None:
    workflow = yaml.load(CORRECTNESS_WORKFLOW.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)
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

    assert jobs["proof-authority-bundle-gate"]["if"] == (
        "github.event_name == 'workflow_dispatch' && inputs.proof_bundle_run_id != ''"
    )
    proof_stage = workflow["on"]["workflow_dispatch"]["inputs"]["proof_stage"]
    assert proof_stage["options"] == ["final", "code"]
    assert proof_stage["default"] == "final"
    bundle_steps = jobs["proof-authority-bundle-gate"]["steps"]
    validation = next(
        step["run"]
        for step in bundle_steps
        if step.get("name") == "Validate the selected qualification stage"
    )
    assert "code) just proof-authority-code-gate ;;" in validation
    assert "final) just proof-authority-release-gate ;;" in validation
    assert jobs["dsl-bench-latency"]["if"] == nightly_or_manual


def test_proof_bundle_dispatch_routes_only_the_selected_stage(tmp_path: Path) -> None:
    workflow = yaml.load(CORRECTNESS_WORKFLOW.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)
    steps = workflow["jobs"]["proof-authority-bundle-gate"]["steps"]
    script = next(
        step["run"]
        for step in steps
        if step.get("name") == "Validate the selected qualification stage"
    )
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    fake_just = fake_bin / "just"
    fake_just.write_text('#!/bin/sh\nprintf "%s\\n" "$1"\n', encoding="utf-8")
    fake_just.chmod(0o755)
    env = os.environ.copy()
    env["PATH"] = f"{fake_bin}:{env['PATH']}"

    for stage, expected in (
        ("code", "proof-authority-code-gate"),
        ("final", "proof-authority-release-gate"),
    ):
        result = subprocess.run(
            ["bash", "-c", script],
            cwd=tmp_path,
            env={**env, "PROOF_STAGE": stage},
            capture_output=True,
            text=True,
        )
        assert result.returncode == 0
        assert result.stdout.strip() == expected

    invalid = subprocess.run(
        ["bash", "-c", script],
        cwd=tmp_path,
        env={**env, "PROOF_STAGE": "unknown"},
        capture_output=True,
        text=True,
    )
    assert invalid.returncode == 2
    assert not invalid.stdout
