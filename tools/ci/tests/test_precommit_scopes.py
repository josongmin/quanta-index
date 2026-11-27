"""Keep local changed-file lint selection aligned with each linter's inputs."""

import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import time
from pathlib import Path

import pytest
import yaml
from pre_commit.git import no_git_env

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[3]
CONFIG = ROOT / ".pre-commit-config.yaml"
WORKFLOW = ROOT / ".github/workflows/ci.yml"
CORRECTNESS_WORKFLOW = ROOT / ".github/workflows/correctness.yml"
CIRCLECI = ROOT / ".circleci/config.yml"
LOCKED_RUST_MANIFESTS = (
    Path("Cargo.toml"),
    Path("shared/quanta-explicit-fd-process/Cargo.toml"),
    Path("shared/quanta-state-root-quota/Cargo.toml"),
    Path("crates/quanta-index-contract/fuzz/Cargo.toml"),
    Path("crates/quanta-index-lq-norm/fuzz/Cargo.toml"),
    Path("tools/ci/ciborium-no-default-consumer/Cargo.toml"),
    Path("tools/ci/tree-sitter-registry-consumer/Cargo.toml"),
    Path("tools/lance-upstream-oracle/Cargo.toml"),
    Path("tools/sqlite-upstream-oracle/Cargo.toml"),
    Path("crates/quanta-index-sqlite-unix-syscall/Cargo.toml"),
    Path("tools/testing/catalog-checked-close/Cargo.toml"),
    Path("crates/quanta-index-sqlite-fd-ledger/Cargo.toml"),
)


def _fixture_git_env() -> dict[str, str]:
    return {
        key: value
        for key, value in no_git_env().items()
        if not key.startswith("PRE_COMMIT_") and key != "SKIP"
    }


def test_local_python_setup_and_scope_hook_use_locked_uv() -> None:
    expected = {
        "install-dev": ["uv", "sync", "--frozen", "--extra", "dev"],
        "install-hooks": [
            "uv",
            "run",
            "--frozen",
            "--extra",
            "dev",
            "pre-commit",
            "install",
            "--install-hooks",
            "--hook-type",
            "pre-commit",
            "--hook-type",
            "pre-push",
        ],
    }
    for recipe, command in expected.items():
        result = subprocess.run(
            ["just", "--dry-run", recipe], cwd=ROOT, capture_output=True, text=True
        )
        assert result.returncode == 0, result.stderr
        assert shlex.split(result.stderr.strip()) == command

    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hook = next(
        hook
        for repo in config["repos"]
        for hook in repo["hooks"]
        if hook["id"] == "precommit-scope-tests"
    )
    assert hook["stages"] == ["pre-push"]
    assert (
        "uv run --frozen --extra dev python -m pytest tools/ci/tests/test_precommit_scopes.py"
        in hook["entry"]
    )


def test_dependency_backed_local_rails_use_locked_uv() -> None:
    for recipe, command in {
        "proof-error-authority-closed": "python tools/ci/check-error-authority-closure.py",
        "precommit-run": "python -m pre_commit run --all-files --show-diff-on-failure",
        "rust-cargo-toml-hygiene": "python tools/ci/lint/check-cargo-toml-hygiene.py",
        "rust-error-shape": "python tools/ci/lint/check-error-shape.py",
        "rust-no-allow": "bash scripts/check-rust-allow-attributes.sh",
        "rust-test-sdk-binding-owner": "python tools/ci/run-local-test-scope.py sdk-binding-owner",
    }.items():
        result = subprocess.run(
            ["just", "--dry-run", recipe], cwd=ROOT, capture_output=True, text=True
        )
        assert result.returncode == 0, result.stderr
        assert f"uv run --frozen --extra dev {command}" in result.stderr

    hooks = {
        hook["id"]: hook
        for repo in yaml.safe_load(CONFIG.read_text(encoding="utf-8"))["repos"]
        for hook in repo["hooks"]
    }
    for hook_id, command in {
        "rust-cargo-toml-hygiene": "python tools/ci/lint/check-cargo-toml-hygiene.py",
        "rust-error-shape": "python tools/ci/lint/check-error-shape.py",
        "rust-no-allow": "bash scripts/check-rust-allow-attributes.sh",
        "prompt-manager-lint": "python tools/prompt-manager/pm.py lint",
    }.items():
        assert hooks[hook_id]["entry"] == f"uv run --frozen --extra dev {command}"
    assert hooks["doc-paths"]["entry"] == "python3 tools/ci/lint/lint-doc-paths.py"


@pytest.mark.parametrize(
    ("hook_id", "violation", "expected"),
    [
        pytest.param("ruff", "import os\nvalue = 1\n", "value = 1\n", id="lint"),
        pytest.param("ruff-format", "value=1\n", "value=1\n", id="format"),
    ],
)
def test_ruff_hooks_check_only_selected_paths(
    tmp_path: Path, hook_id: str, violation: str, expected: str
) -> None:
    hooks = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))["repos"]
    hook = next(hook for repo in hooks for hook in repo["hooks"] if hook["id"] == hook_id)
    assert hook.get("pass_filenames", True)
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    (scripts / "quanta-index-env.sh").write_text(
        'uv() { shift 5; exec "$RUFF_TEST_PYTHON" "$@"; }\n', encoding="utf-8"
    )
    selected = tmp_path / "-selected with spaces.py"
    selected.write_text("value = 1\n", encoding="utf-8")
    unrelated = tmp_path / "unrelated.py"
    unrelated.write_text(violation, encoding="utf-8")
    command = [*shlex.split(hook["entry"]), selected.name]
    env = {**os.environ, "RUFF_TEST_PYTHON": sys.executable}

    clean = subprocess.run(command, cwd=tmp_path, env=env, capture_output=True, text=True)

    assert clean.returncode == 0, clean.stdout + clean.stderr
    assert unrelated.read_text(encoding="utf-8") == violation

    selected.write_text(violation, encoding="utf-8")
    dirty = subprocess.run(command, cwd=tmp_path, env=env, capture_output=True, text=True)

    assert dirty.returncode == 1, dirty.stdout + dirty.stderr
    assert selected.read_text(encoding="utf-8") == expected
    assert unrelated.read_text(encoding="utf-8") == violation


def write_lock_freshness_fixtures(root: Path, *, missing: str | None = None) -> None:
    for lockfile in (
        Path("uv.lock"),
        *(manifest.with_name("Cargo.lock") for manifest in LOCKED_RUST_MANIFESTS),
    ):
        if lockfile.as_posix() != missing:
            path = root / lockfile
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()


def test_github_actions_has_no_automatic_trigger_and_circleci_is_default() -> None:
    for path in (WORKFLOW, CORRECTNESS_WORKFLOW, ROOT / ".github/workflows/potion-code-parity.yml"):
        workflow = yaml.load(path.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)
        assert set(workflow["on"]) == {"workflow_dispatch"}
    config = yaml.safe_load(CIRCLECI.read_text(encoding="utf-8"))
    assert config["parameters"]["run_heavy"]["default"] is False
    assert "regular" in config["workflows"]
    assert config["workflows"]["manual-heavy"]["jobs"] == ["heavy-correctness"]


def test_shellcheck_covers_the_canonical_cargo_wrapper(tmp_path: Path) -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hook = next(
        hook for repo in config["repos"] for hook in repo["hooks"] if hook["id"] == "shellcheck"
    )
    assert re.search(hook["files"], "scripts/cargow")
    assert re.search(hook["files"], "scripts/rust-workspaces.sh")

    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    fake_uv = fake_bin / "uv"
    fake_uv.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n', encoding="utf-8")
    fake_uv.chmod(0o755)
    env = os.environ.copy()
    env["PATH"] = f"{fake_bin}:{env['PATH']}"
    env["TMPDIR"] = str(tmp_path)
    result = subprocess.run(
        ["bash", str(ROOT / "scripts/run-shellcheck.sh")],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    command = result.stdout.splitlines()
    prefix = [
        "run",
        "--frozen",
        "--extra",
        "dev",
        "python",
        "tools/ci/run-read-only-hook.py",
        "shellcheck",
    ]
    assert command[: len(prefix)] == prefix
    assert "scripts/cargow" in command[len(prefix) :]
    assert list(tmp_path.glob("quanta-shell-lint.*")) == []


def test_shellcheck_inventory_failure_cannot_pass(tmp_path: Path) -> None:
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    fake_find = fake_bin / "find"
    fake_find.write_text("#!/bin/sh\necho 'simulated find failure' >&2\nexit 23\n")
    fake_find.chmod(0o755)
    fake_uv = fake_bin / "uv"
    fake_uv.write_text("#!/bin/sh\necho 'shellcheck invoked'\n")
    fake_uv.chmod(0o755)
    env = os.environ.copy()
    env["PATH"] = f"{fake_bin}:{env['PATH']}"
    env["TMPDIR"] = str(tmp_path)

    result = subprocess.run(
        ["bash", str(ROOT / "scripts/run-shellcheck.sh")],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
    )

    assert result.returncode != 0
    assert "simulated find failure" in result.stderr
    assert "shellcheck invoked" not in result.stdout
    assert list(tmp_path.glob("quanta-shell-lint.*")) == []


def test_shellcheck_rejects_non_executable_cargow(tmp_path: Path) -> None:
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    wrapper = scripts / "run-shellcheck.sh"
    wrapper.write_bytes((ROOT / "scripts/run-shellcheck.sh").read_bytes())
    cargow = scripts / "cargow"
    cargow.write_text("#!/bin/bash\nexit 0\n", encoding="utf-8")
    cargow.chmod(0o644)
    (tmp_path / "tools/ci/lint").mkdir(parents=True)
    fake_uv = scripts / "uv"
    fake_uv.write_text("#!/bin/sh\necho 'shellcheck invoked'\n", encoding="utf-8")
    fake_uv.chmod(0o755)

    result = subprocess.run(
        ["bash", str(wrapper)],
        cwd=tmp_path,
        env={**os.environ, "PATH": f"{scripts}:{os.environ['PATH']}", "TMPDIR": str(tmp_path)},
        capture_output=True,
        text=True,
    )

    assert result.returncode == 1, result.stdout + result.stderr
    assert "scripts/cargow is not executable" in result.stderr
    assert "shellcheck invoked" not in result.stdout


@pytest.mark.parametrize(("edited", "expected"), [("unrelated.md", 0), ("selected.sh", 1)])
def test_read_only_hook_checks_for_changes_to_its_inputs(
    tmp_path: Path, edited: str, expected: int
) -> None:
    env = _fixture_git_env()
    subprocess.run(["git", "init", "-q", str(tmp_path)], env=env, check=True)
    (tmp_path / "selected.sh").write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    (tmp_path / "unrelated.md").write_text("before\n", encoding="utf-8")
    (tmp_path / "lint.py").write_text(
        "import os, sys, time\nfrom pathlib import Path\n"
        'assert os.environ["PRE_COMMIT"] == "1"\n'
        'assert sys.argv[1:] == ["--fixture", "selected.sh"]\n'
        'assert Path("selected.sh").read_text().startswith("#!/bin/sh")\n'
        'Path("ready").touch()\n'
        "deadline = time.monotonic() + 60\n"
        'while not Path("release").exists() and time.monotonic() < deadline: time.sleep(.02)\n'
        'assert Path("release").exists()\n',
        encoding="utf-8",
    )
    config = {
        "repos": [
            {
                "repo": "local",
                "hooks": [
                    {
                        "id": "shellcheck",
                        "name": "fixture shellcheck",
                        "entry": shlex.join([sys.executable, "lint.py"]),
                        "language": "system",
                        "files": r"^selected\.sh$",
                        "args": ["--fixture"],
                    }
                ],
            }
        ]
    }
    (tmp_path / ".pre-commit-config.yaml").write_text(yaml.safe_dump(config), encoding="utf-8")
    subprocess.run(["git", "add", "."], cwd=tmp_path, env=env, check=True)

    process = subprocess.Popen(
        [
            sys.executable,
            str(ROOT / "tools/ci/run-read-only-hook.py"),
            "shellcheck",
            "selected.sh",
            "unrelated.md",
        ],
        cwd=tmp_path,
        env={**env, "PRE_COMMIT_HOME": str(tmp_path / "hook-cache")},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    ready = tmp_path / "ready"
    try:
        deadline = time.monotonic() + 30
        while not ready.exists() and process.poll() is None and time.monotonic() < deadline:
            time.sleep(0.01)
        assert ready.exists(), "configured linter did not start"
        with (tmp_path / edited).open("a", encoding="utf-8") as stream:
            stream.write("parallel edit\n")
    finally:
        (tmp_path / "release").touch()
        try:
            stdout, stderr = process.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate()
            raise

    assert process.returncode == expected, stdout + stderr
    assert ("Lint input changed" in stderr) == bool(expected)


@pytest.mark.parametrize(("recipe", "suffix"), [("fmt", ""), ("fmt-check", " -- --check")])
def test_fmt_recipes_visit_each_owned_workspace(tmp_path: Path, recipe: str, suffix: str) -> None:
    root = tmp_path / "checkout"
    root.mkdir()
    (root / "Justfile").write_bytes((ROOT / "Justfile").read_bytes())
    wrapper = root / "scripts/cargow"
    wrapper.parent.mkdir()
    wrapper.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$CARGO_CALL_LOG"\n')
    wrapper.chmod(0o755)
    (root / "scripts/run-rustfmt.sh").write_bytes((ROOT / "scripts/run-rustfmt.sh").read_bytes())
    (root / "scripts/rust-workspaces.sh").write_bytes(
        (ROOT / "scripts/rust-workspaces.sh").read_bytes()
    )
    log = tmp_path / "cargo-calls"
    env = os.environ.copy()
    env["CARGO_CALL_LOG"] = str(log)

    result = subprocess.run(["just", recipe], cwd=root, env=env, capture_output=True, text=True)

    assert result.returncode == 0, result.stderr
    assert log.read_text().splitlines() == [
        f"--lane fmt-lane fmt{suffix}",
        *(
            f"--lane fmt-lane fmt --manifest-path {manifest}{suffix}"
            for manifest in LOCKED_RUST_MANIFESTS[1:]
        ),
    ]


@pytest.mark.parametrize(
    "failing_manifest",
    [Path("Cargo.toml"), Path("shared/quanta-explicit-fd-process/Cargo.toml")],
)
def test_fmt_check_reports_failure_after_visiting_all_owned_workspaces(
    tmp_path: Path, failing_manifest: Path
) -> None:
    root = tmp_path / "checkout"
    root.mkdir()
    (root / "Justfile").write_bytes((ROOT / "Justfile").read_bytes())
    wrapper = root / "scripts/cargow"
    wrapper.parent.mkdir()
    wrapper.write_text(
        '#!/bin/sh\nprintf "%s\\n" "$*" >> "$CARGO_CALL_LOG"\n'
        'if [ "$FMT_FAIL_MANIFEST" = Cargo.toml ]; then\n'
        '  case " $* " in *" --manifest-path "*) ;; *) exit 23;; esac\n'
        "else\n"
        '  case " $* " in *" --manifest-path $FMT_FAIL_MANIFEST "*) exit 23;; esac\n'
        "fi\n"
    )
    wrapper.chmod(0o755)
    (root / "scripts/run-rustfmt.sh").write_bytes((ROOT / "scripts/run-rustfmt.sh").read_bytes())
    (root / "scripts/rust-workspaces.sh").write_bytes(
        (ROOT / "scripts/rust-workspaces.sh").read_bytes()
    )
    log = tmp_path / "cargo-calls"
    env = os.environ.copy()
    env["CARGO_CALL_LOG"] = str(log)
    env["FMT_FAIL_MANIFEST"] = str(failing_manifest)

    result = subprocess.run(
        ["just", "fmt-check"], cwd=root, env=env, capture_output=True, text=True
    )

    assert result.returncode != 0
    assert len(log.read_text().splitlines()) == len(LOCKED_RUST_MANIFESTS)


def test_fmt_hook_and_manual_ci_use_the_canonical_runner_for_owned_sources() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hook = next(
        hook
        for repo in config["repos"]
        for hook in repo["hooks"]
        if hook["id"] == "cargo-fmt-check"
    )
    runner = "bash scripts/run-rustfmt.sh check"
    assert hook["entry"] == runner
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    fmt_steps = workflow["jobs"]["rust-fmt"]["steps"]
    assert next(step["run"] for step in fmt_steps if step.get("name") == "rust fmt") == runner
    selected = re.compile(hook["files"])
    for path in (
        "Justfile",
        ".pre-commit-config.yaml",
        "scripts/cargow",
        "scripts/run-rustfmt.sh",
        "scripts/rust-workspaces.sh",
        "shared/quanta-explicit-fd-process/src/lib.rs",
        "shared/quanta-explicit-fd-process/Cargo.toml",
        "shared/quanta-installed-artifact-trust/src/lib.rs",
        "crates/quanta-index-contract/fuzz/fuzz_targets/ipc_request_decode.rs",
        "tools/ci/ciborium-no-default-consumer/src/lib.rs",
        "tools/ci/tree-sitter-registry-consumer/src/lib.rs",
        "tools/lance-upstream-oracle/src/lib.rs",
        "tools/sqlite-upstream-oracle/src/lib.rs",
        "tools/testing/catalog-checked-close/tests/checked_close.rs",
        "crates/quanta-index-sqlite-fd-ledger/src/lib.rs",
    ):
        assert selected.fullmatch(path), path
    assert not selected.fullmatch("README.md")


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
            "tools/ci/lint/check-module-cycles.py",
        ),
        "rust-read-view-ambient": (
            "crates/quanta-index-search-plane/src/query_dispatcher/routes/query.rs",
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-read-view-ambient-lookup.py",
            "tools/ci/tests/test_check_read_view_ambient_lookup.py",
        ),
        "rust-no-allow": (
            "Cargo.toml",
            "crates/quanta-index-core/src/lib.rs",
            "shared/quanta-installed-artifact-trust/src/lib.rs",
            "scripts/check-rust-allow-attributes.sh",
            "tools/ci/lint/check-rust-allow-attributes.py",
            "tools/ci/lint/rust_attribute_policy.py",
            "tools/ci/tests/test_check_rust_allow_attributes.py",
        ),
        "rust-derive-allowlist": (
            "crates/quanta-index-core/src/lib.rs",
            "shared/quanta-installed-artifact-trust/src/lib.rs",
            "tools/ci/lint/check-rust-derive-allowlist.py",
            "tools/ci/lint/rust_attribute_policy.py",
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
        "rust-digest-fallibility": (
            "crates/quanta-index-core/src/lib.rs",
            "tools/ci/lint/check-digest-fallibility.py",
        ),
        "semgrep": (
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
        "rust-fallbacks": (
            "crates/quanta-index-core/src/lib.rs",
            "crates/quanta-index-search-plane/src/lib.rs",
            "crates/quanta-index-core/fuzz/fuzz_targets/silent.rs",
            "benchmarks/retrieval/src/lib.rs",
            "tools/ci/lint/check-rust-fallbacks.py",
            "tools/ci/lint/rust_attribute_policy.py",
        ),
        "rust-fallback-tests": (
            "tools/ci/lint/check-rust-fallbacks.py",
            "tools/ci/lint/rust_attribute_policy.py",
            "tools/ci/tests/test_check_rust_fallbacks.py",
        ),
        "precommit-scope-tests": (
            ".pre-commit-config.yaml",
            ".circleci/config.yml",
            ".github/workflows/ci.yml",
            ".github/workflows/correctness.yml",
            ".github/workflows/potion-code-parity.yml",
            "Justfile",
            "pyproject.toml",
            "uv.lock",
            "scripts/cargow",
            "scripts/check-lock-freshness.sh",
            "scripts/run-actionlint.sh",
            "scripts/run-cargo-deny.sh",
            "scripts/run-rustfmt.sh",
            "scripts/run-shellcheck.sh",
            "scripts/rust-workspaces.sh",
            "tools/ci/run-hooks-on-deletions.py",
            "tools/ci/run-read-only-hook.py",
            "tools/ci/tests/test_precommit_scopes.py",
            "tools/ci/tests/test_run_actionlint.py",
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


def test_authority_hooks_include_changes_with_no_remaining_file() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {
        hook["id"]: hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
    }
    for hook_id, stages in {
        "test-authority": ["pre-commit", "pre-push"],
        "proof-authority": ["pre-commit", "pre-push"],
        "rust-wire-inventory": ["pre-push"],
        "rust-error-code-table": ["pre-push"],
        "rust-ignored-test-policy": ["pre-push"],
    }.items():
        assert hooks[hook_id].get("always_run") is True, hook_id
        assert hooks[hook_id]["pass_filenames"] is False
        assert hooks[hook_id]["stages"] == stages
    assert hooks["rust-error-code-table"]["entry"] == "just rust-error-code-table"
    assert hooks["rust-error-code-table"]["language"] == "system"


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
        ".semgrepignore",
        "tools/ci/semgrep/rules.yml",
        "tools/ci/tests/test_semgrep_policy.py",
    ):
        assert not generic.search(path), path
        assert dedicated.search(path), path
    assert generic.search("scripts/run-tooling-tests.sh")
    assert generic.search("scripts/quanta-index-env.sh")
    assert generic.search("uv.lock")
    for scope_test in (
        "tools/ci/tests/test_precommit_scopes.py",
        "tools/ci/tests/test_run_actionlint.py",
    ):
        assert not generic.search(scope_test)
        assert f"--ignore={scope_test}" in hooks["prompt-manager-tests"]["entry"]
        assert scope_test in hooks["precommit-scope-tests"]["entry"]
    for scope_runner in ("tools/ci/run-read-only-hook.py", "tools/ci/run-hooks-on-deletions.py"):
        assert not generic.search(scope_runner)
    assert not generic.search("tools/ci/tests/test_check_rust_fallbacks.py")
    assert not generic.search("tools/ci/lint/check-rust-fallbacks.py")
    assert re.compile(hooks["rust-fallback-tests"]["files"]).search(
        "tools/ci/tests/test_check_rust_fallbacks.py"
    )
    assert not generic.search(".pre-commit-config.yaml")
    scoped = re.compile(hooks["precommit-scope-tests"]["files"])
    assert scoped.search("tools/ci/tests/test_precommit_scopes.py")
    assert scoped.search(".pre-commit-config.yaml")
    assert "tools/ci/tests/test_precommit_scopes.py" in hooks["precommit-scope-tests"]["entry"]
    scanner = re.compile(hooks["semgrep"]["files"])
    assert re.compile(hooks["rust-semantic-outcomes"]["files"]).search(
        "crates/quanta-index-searchd/src/lib.rs"
    )
    for irrelevant in (
        "scripts/check-semgrep-rules.sh",
        "crates/quanta-index-searchd/src/lib.rs",
        "crates/quanta-index-core/src/lib.rs",
        "crates/quanta-index-search-plane/src/lib.rs",
        "benchmarks/retrieval/src/lib.rs",
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
    assert "--ignore=tools/ci/tests/test_check_rust_fallbacks.py" in runner
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    verify = justfile.split("verify:\n", 1)[1].split("\n\n", 1)[0]
    assert verify.count("@just semgrep-rule-tests") == 1
    assert verify.count("@just semgrep\n") == 1


def test_rust_verification_runs_semantic_and_fallback_policy_once() -> None:
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    full_verification = justfile.split("verify:\n", 1)[1].split("\n\n", 1)[0].splitlines()
    rust_verification = justfile.split("\nverify-rust:", 1)[1].split("\n\n", 1)[0].splitlines()
    assert full_verification.count("    @just verify-rust") == 1
    for command in ("    @just rust-semantic-outcomes", "    @just rust-fallbacks"):
        assert rust_verification.count(command) == 1
        assert command not in full_verification


def test_local_benchmark_policy_uses_locked_env_and_precedes_benchmark_work() -> None:
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    policy = subprocess.run(
        ["just", "--dry-run", "benchmark-policy-local"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert policy.returncode == 0, policy.stderr
    assert policy.stderr.strip() == (
        "uv run --frozen --extra dev python "
        "tools/ci/lint/check-benchmark-policy.py --print-registry-digest"
    )

    rust = justfile.split("\nverify-rust:", 1)[1].split("\n\n", 1)[0].splitlines()
    prep = justfile.split("benchmark-prep-local:\n", 1)[1].split("\n\n", 1)[0].splitlines()
    policy_call = "    @just benchmark-policy-local"
    assert rust.count(policy_call) == 1
    assert rust.index(policy_call) < rust.index("    @just rust-bench-build")
    assert prep.count(policy_call) == 1
    assert prep.index(policy_call) < prep.index(
        "    uv run --frozen --extra dev just benchmark-control-contract-local"
    )


def test_benchmark_replay_preserves_the_run_path(tmp_path: Path) -> None:
    run_dir = str(tmp_path / "stored benchmark run")
    result = subprocess.run(
        ["just", "--dry-run", "benchmark-replay", run_dir],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    assert shlex.split(result.stderr) == [
        "uv",
        "run",
        "--frozen",
        "--extra",
        "dev",
        "python",
        "tools/benchmark/benchctl.py",
        "replay",
        run_dir,
    ]


def test_local_handoff_checks_use_locked_python(tmp_path: Path) -> None:
    receipt = str(tmp_path / "lane handoff.json")
    for recipe, arguments, expected in (
        ("lane-handoff-check", [receipt], ["--require-result-head", receipt]),
        ("lane-handoff-check-historical", [receipt], [receipt]),
        ("lane-handoff-chain-check", [], ["--product-chain", "artifacts/sep-21/handoffs"]),
    ):
        result = subprocess.run(
            ["just", "--dry-run", recipe, *arguments],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        assert result.returncode == 0, result.stderr
        assert shlex.split(result.stderr) == [
            "uv",
            "run",
            "--frozen",
            "--extra",
            "dev",
            "python",
            "tools/ci/lint/check-lane-handoff.py",
            *expected,
        ]


def test_local_p00_writes_and_collects_proof_with_locked_python() -> None:
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    python = "uv run --frozen --extra dev python"
    inventory = justfile.split("proof-error-authority-inventory:\n", 1)[1].split("\n\n", 1)[0]
    assert f"{python} tools/ci/write-error-authority-inventory.py" in inventory
    proof = justfile.split("proof-p00-authority-freeze:\n", 1)[1].split("\n\n", 1)[0]
    assert "    @just proof-error-authority-inventory\n" in proof
    assert f"{python} tools/ci/proof_execution_result.py collect-pytest" in proof
    assert f"    {python} -m pytest" in proof


def test_cargo_hooks_include_shared_build_configuration() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {
        hook["id"]: hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
    }
    assert hooks["cargo-test"]["entry"] == "just rust-test"
    for hook_id in (
        "cargo-fmt-check",
        "cargo-check",
        "cargo-clippy-strict",
        "cargo-test",
        "cargo-doc",
    ):
        pattern = re.compile(hooks[hook_id]["files"])
        for path in (
            "rust-toolchain.toml",
            ".cargo/config.toml",
            "scripts/cargow",
            "scripts/quanta-index-env.sh",
            "crates/quanta-index-sqlite-fd-ledger/Cargo.lock",
        ):
            assert pattern.search(path), (hook_id, path)
        assert not pattern.search("docs/reference/cargo-config.toml"), hook_id


def test_default_rust_test_selects_the_standalone_sqlite_fd_ledger() -> None:
    default = subprocess.check_output(
        ["just", "--dry-run", "rust-test"], cwd=ROOT, stderr=subprocess.STDOUT, text=True
    )
    assert "just rust-test-sqlite-fd-ledger" in default
    ledger = subprocess.check_output(
        ["just", "--dry-run", "rust-test-sqlite-fd-ledger"],
        cwd=ROOT,
        stderr=subprocess.STDOUT,
        text=True,
    )
    assert (
        "nextest run --manifest-path crates/quanta-index-sqlite-fd-ledger/Cargo.toml "
        "--lib --locked --offline --no-tests fail"
    ) in ledger


@pytest.mark.parametrize(
    ("hook_id", "recipe", "subcommand", "target_flags"),
    [
        ("cargo-check", "rust-check", "check", "--all-targets --all-features"),
        ("cargo-clippy-strict", "rust-clippy", "clippy", "--all-targets --all-features"),
        ("cargo-doc", "rust-doc", "doc", "--all-features --no-deps"),
    ],
)
def test_rust_quality_hooks_check_the_standalone_sqlite_fd_ledger(
    hook_id: str, recipe: str, subcommand: str, target_flags: str
) -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {hook["id"]: hook for repo in config["repos"] for hook in repo["hooks"]}
    assert hooks[hook_id]["entry"] == f"just {recipe}"
    commands = subprocess.check_output(
        ["just", "--dry-run", recipe], cwd=ROOT, stderr=subprocess.STDOUT, text=True
    )
    assert f"{subcommand} --workspace {target_flags} --locked" in commands
    assert (
        f"{subcommand} --manifest-path crates/quanta-index-sqlite-fd-ledger/Cargo.toml "
        f"{target_flags} --locked --offline"
    ) in commands


def test_cargow_behavioral_suites_are_selected_by_wrapper_and_admission_changes() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {
        hook["id"]: hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
    }
    behavior = hooks["cargow-tests"]
    broad = hooks["prompt-manager-tests"]
    sources = (
        "scripts/cargow",
        "scripts/quanta-index-env.sh",
        "tools/ci/resource_admission.py",
        "tools/benchmark/producer_execution.py",
    )
    suites = (
        "tools/ci/tests/test_cargow_lane_env.py",
        "tools/ci/tests/test_cargow_resource_admission.py",
        "tools/ci/tests/test_resource_admission.py",
    )
    assert behavior["stages"] == ["pre-push"]
    assert all(re.search(behavior["files"], path) for path in (*sources, *suites))
    assert not re.search(behavior["files"], "docs/analysis/unrelated.md")
    for suite in suites:
        assert suite in behavior["entry"]
        assert f"--ignore={suite}" in broad["entry"]
        assert not re.search(broad["files"], suite)


@pytest.mark.parametrize(
    ("stage", "regular_ids", "fallback_id", "input_path"),
    [
        (
            "pre-commit",
            ("cargo-fmt-check", "cargo-check"),
            "pre-commit-deletions",
            "crates/demo/src/dependency.rs",
        ),
        (
            "pre-push",
            ("cargo-clippy-strict", "cargo-test", "cargo-doc"),
            "pre-push-deletions",
            "crates/demo/src/dependency.rs",
        ),
        pytest.param(
            "pre-push",
            ("prompt-manager-tests",),
            "pre-push-deletions",
            "tools/benchmark/retrieval/proof_inventory.py",
            id="python-tooling",
        ),
        pytest.param(
            "pre-push",
            ("precommit-scope-tests",),
            "pre-push-deletions",
            "scripts/run-rustfmt.sh",
            id="hook-wrapper",
        ),
    ],
)
@pytest.mark.parametrize("change", ["delete", "docs", "mixed", "rename", "skip", "failure"])
def test_deleted_inputs_use_the_configured_repository_gate_once(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    stage: str,
    regular_ids: tuple[str, ...],
    fallback_id: str,
    input_path: str,
    change: str,
) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    calls = tmp_path / "calls"
    outer_git = tmp_path / "outer.git"
    monkeypatch.setenv("GIT_DIR", str(outer_git))
    monkeypatch.setenv("PRE_COMMIT_FROM_REF", "a" * 40)
    monkeypatch.setenv("PRE_COMMIT_TO_REF", "b" * 40)
    env = _fixture_git_env()
    env["PRE_COMMIT_HOME"] = str(tmp_path / "precommit")
    env["SKIP"] = regular_ids[0] if change == "skip" else ""

    def git(*args: str) -> str:
        return subprocess.check_output(
            [
                "git",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                *args,
            ],
            cwd=root,
            env=env,
            text=True,
        ).strip()

    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {
        hook["id"]: hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
    }
    for hook_id in regular_ids:
        code = (
            "from pathlib import Path; import sys\n"
            "with Path(sys.argv[1]).open('a') as calls: calls.write(sys.argv[2] + '\\n')\n"
            "raise SystemExit(int(sys.argv[3]))"
        )
        exit_code = "9" if change == "failure" and hook_id == regular_ids[0] else "0"
        hooks[hook_id]["entry"] = shlex.join(
            [sys.executable, "-c", code, str(calls), hook_id, exit_code]
        )
    script = ROOT / "tools/ci/run-hooks-on-deletions.py"
    hooks[fallback_id]["entry"] = shlex.join([sys.executable, str(script), stage])
    (root / ".pre-commit-config.yaml").write_text(
        yaml.safe_dump(
            {"repos": [{"repo": "local", "hooks": [hooks[i] for i in (*regular_ids, fallback_id)]}]}
        ),
        encoding="utf-8",
    )
    source = Path(input_path)
    (root / source).parent.mkdir(parents=True)
    (root / source).write_text("initial input\n", encoding="utf-8")
    (root / source.with_stem("retained")).write_text("retained input\n", encoding="utf-8")
    (root / "README.md").write_text("old\n", encoding="utf-8")
    git("init", "-q")
    git("add", ".")
    git("commit", "-qm", "initial inputs")
    before = git("rev-parse", "HEAD")
    if change == "docs":
        (root / "README.md").write_text("new\n", encoding="utf-8")
    elif change == "rename":
        git("mv", str(source), str(source.with_stem("replacement")))
    else:
        git("rm", "-q", str(source))
        if change == "mixed":
            (root / source.with_stem("added")).write_text("added input\n", encoding="utf-8")
    git("add", ".")
    command = [sys.executable, "-m", "pre_commit", "run", "--hook-stage", stage]
    if stage == "pre-push":
        git("commit", "-qm", "updated inputs")
        command.extend(["--from-ref", before, "--to-ref", "HEAD"])
    result = subprocess.run(command, cwd=root, env=env, text=True, capture_output=True, check=False)

    assert not outer_git.exists()
    assert result.returncode == int(change == "failure"), result.stdout + result.stderr
    actual = calls.read_text(encoding="utf-8").splitlines() if calls.exists() else []
    expected = [] if change == "docs" else list(regular_ids)
    if change == "skip":
        expected.remove(regular_ids[0])
    assert actual == expected, result.stdout + result.stderr


def test_benchmark_source_and_manifest_select_cargo_hooks() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {
        hook["id"]: hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
    }
    for hook_id in (
        "cargo-fmt-check",
        "cargo-check",
        "cargo-clippy-strict",
        "cargo-test",
        "cargo-doc",
    ):
        pattern = re.compile(hooks[hook_id]["files"])
        assert pattern.search("benchmarks/bench-protocol/Cargo.toml"), hook_id
        assert pattern.search("benchmarks/bench-protocol/src/lib.rs"), hook_id
    assert re.compile(hooks["rust-cargo-toml-hygiene"]["files"]).search(
        "benchmarks/bench-protocol/Cargo.toml"
    )
    deny_hook = hooks["cargo-deny"]
    deny_command = "bash scripts/run-cargo-deny.sh --all-workspaces"
    assert deny_hook["entry"] == deny_command
    jobs = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))["jobs"]
    assert any(step.get("run") == deny_command for step in jobs["rust-deps"]["steps"])
    deny = re.compile(deny_hook["files"])
    for path in (
        "Cargo.toml",
        "Cargo.lock",
        "deny.toml",
        "rust-toolchain.toml",
        ".cargo/config.toml",
        "scripts/run-cargo-deny.sh",
        "scripts/quanta-index-env.sh",
        "crates/quanta-index-searchd/Cargo.toml",
        "benchmarks/retrieval/Cargo.toml",
        "benchmarks/bench-protocol/Cargo.toml",
        "shared/quanta-installed-artifact-trust/Cargo.toml",
        "shared/quanta-state-root-quota/Cargo.toml",
        "shared/quanta-state-root-quota/Cargo.lock",
        "tools/lance-upstream-oracle/Cargo.lock",
        "scripts/rust-workspaces.sh",
    ):
        assert deny.fullmatch(path), path

    machete = re.compile(hooks["cargo-machete"]["files"])
    for path in ("Cargo.toml", "Cargo.lock"):
        assert machete.fullmatch(path), path
    for member in (
        "crates/quanta-index-searchd",
        "benchmarks/retrieval",
        "benchmarks/bench-protocol",
        "shared/quanta-installed-artifact-trust",
    ):
        for suffix in (
            "Cargo.toml",
            "build.rs",
            "src/lib.rs",
            "tests/fixture.rs",
            "benches/case.rs",
        ):
            path = f"{member}/{suffix}"
            assert machete.fullmatch(path), path
    for pattern in (deny, machete):
        assert not pattern.search("docs/analysis/unrelated.md")
        assert not pattern.search("tools/ci/tests/test_precommit_scopes.py")
    assert not deny.fullmatch("vendor/lance-7.0.0/Cargo.toml")
    members = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
        "members"
    ]
    for member in members:
        assert deny.fullmatch(f"{member}/Cargo.toml"), member
        for suffix in ("Cargo.toml", "build.rs", "src/lib.rs", "tests/fixture.rs"):
            assert machete.fullmatch(f"{member}/{suffix}"), (member, suffix)


@pytest.mark.parametrize(
    ("audit_all", "rejected"),
    [
        (False, None),
        (True, None),
        (True, Path("shared/quanta-state-root-quota/Cargo.toml")),
    ],
)
def test_cargo_deny_checks_locked_workspace_graphs(
    tmp_path: Path, audit_all: bool, rejected: Path | None
) -> None:
    checkout = tmp_path / "checkout with spaces"
    scripts = checkout / "scripts"
    scripts.mkdir(parents=True)
    for name in ("run-cargo-deny.sh", "rust-workspaces.sh"):
        (scripts / name).write_bytes((ROOT / "scripts" / name).read_bytes())
    (scripts / "quanta-index-env.sh").touch()
    (checkout / "deny.toml").touch()
    for manifest in LOCKED_RUST_MANIFESTS:
        path = checkout / manifest
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()
        path.with_name("Cargo.lock").touch()

    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    (bin_dir / "cargo-deny").touch(mode=0o700)
    (bin_dir / "cargo").write_text(
        "#!/usr/bin/env python3\n"
        "import json, os, sys\n"
        "args = sys.argv[1:]\n"
        "with open(os.environ['QIDX_DENY_CALLS'], 'a') as out:\n"
        "    out.write(json.dumps(args) + '\\n')\n"
        "manifest = args[args.index('--manifest-path') + 1]\n"
        "raise SystemExit(43 if manifest == os.environ['QIDX_DENY_REJECT'] else 0)\n",
        encoding="utf-8",
    )
    (bin_dir / "cargo").chmod(0o755)
    calls = tmp_path / "deny-calls"
    args = ["--all-workspaces"] if audit_all else []
    result = subprocess.run(
        ["bash", str(scripts / "run-cargo-deny.sh"), *args, "--hide-inclusion-graph"],
        cwd=tmp_path,
        env={
            **os.environ,
            "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
            "QIDX_DENY_CALLS": str(calls),
            "QIDX_DENY_REJECT": str(checkout / rejected) if rejected else "",
        },
        capture_output=True,
        text=True,
    )

    assert result.returncode == (43 if rejected else 0), result.stderr
    if rejected:
        assert f"cargo-deny failed for {rejected} (exit 43)" in result.stderr
    manifests = LOCKED_RUST_MANIFESTS if audit_all else LOCKED_RUST_MANIFESTS[:1]
    commands = [json.loads(line) for line in calls.read_text(encoding="utf-8").splitlines()]
    for command, manifest in zip(commands, manifests, strict=True):
        assert command[0] == "deny"
        assert {"--locked", "--workspace"} <= set(command[: command.index("check")])
        assert command[command.index("--manifest-path") + 1] == str(checkout / manifest)
        config = command.index("--config")
        assert command[config + 1] == str(checkout / "deny.toml")
        checks = {"advisories", "licenses", "sources", "--hide-inclusion-graph"}
        if manifest == Path("Cargo.toml"):
            checks.add("bans")
        assert set(command[config + 2 :]) == checks


@pytest.mark.parametrize(
    ("cargo_exit", "uv_exit", "expected"), [(0, 0, 0), (41, 0, 41), (0, 42, 42)]
)
def test_lock_freshness_checks_current_inputs_without_staged_files(
    tmp_path: Path, cargo_exit: int, uv_exit: int, expected: int
) -> None:
    env = _fixture_git_env()
    subprocess.run(["git", "init", "-q", str(tmp_path)], env=env, check=True)
    write_lock_freshness_fixtures(tmp_path)
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    (scripts / "rust-workspaces.sh").write_bytes((ROOT / "scripts/rust-workspaces.sh").read_bytes())
    for tool, status in (("cargow", cargo_exit), ("uv", uv_exit)):
        stub = scripts / tool
        stub.write_text(f"#!/bin/sh\nprintf '%s\\n' \"{tool} $*\" >> calls\nexit {status}\n")
        stub.chmod(0o755)

    result = subprocess.run(
        ["bash", str(ROOT / "scripts/check-lock-freshness.sh")],
        cwd=tmp_path,
        env={**env, "PATH": f"{scripts}:{env['PATH']}"},
        check=False,
    )

    assert result.returncode == expected
    manifests = LOCKED_RUST_MANIFESTS[:1] if cargo_exit != 0 else LOCKED_RUST_MANIFESTS
    calls = [
        f"cargow metadata --manifest-path {manifest} --format-version 1 --locked"
        for manifest in manifests
    ]
    if cargo_exit == 0:
        calls.append("uv lock --check")
    assert (tmp_path / "calls").read_text().splitlines() == calls


@pytest.mark.parametrize(
    "missing",
    [
        "Cargo.lock",
        "uv.lock",
        "shared/quanta-state-root-quota/Cargo.lock",
        "crates/quanta-index-contract/fuzz/Cargo.lock",
        "tools/ci/ciborium-no-default-consumer/Cargo.lock",
        "tools/ci/tree-sitter-registry-consumer/Cargo.lock",
        "tools/lance-upstream-oracle/Cargo.lock",
        "tools/sqlite-upstream-oracle/Cargo.lock",
        "tools/testing/catalog-checked-close/Cargo.lock",
    ],
)
def test_lock_freshness_rejects_a_deleted_lockfile(tmp_path: Path, missing: str) -> None:
    hooks = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))["repos"]
    lock_hook = next(
        hook
        for repo in hooks
        if repo["repo"] == "local"
        for hook in repo["hooks"]
        if hook["id"] == "lock-freshness"
    )
    config = {"repos": [{"repo": "local", "hooks": [lock_hook]}]}
    (tmp_path / ".pre-commit-config.yaml").write_text(yaml.safe_dump(config))
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    (scripts / "check-lock-freshness.sh").write_text(
        (ROOT / "scripts/check-lock-freshness.sh").read_text()
    )
    (scripts / "rust-workspaces.sh").write_bytes((ROOT / "scripts/rust-workspaces.sh").read_bytes())
    write_lock_freshness_fixtures(tmp_path, missing=missing)
    env = _fixture_git_env()
    subprocess.run(["git", "init", "-q", str(tmp_path)], env=env, check=True)

    result = subprocess.run(
        [sys.executable, "-m", "pre_commit", "run", "lock-freshness", "--files", missing],
        cwd=tmp_path,
        env={**env, "PRE_COMMIT_HOME": str(tmp_path / "pre-commit-cache")},
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 1, result.stdout + result.stderr
    assert f"Missing required lockfile: {missing}" in result.stdout


def test_workspace_members_select_local_policy_hooks() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hooks = {
        hook["id"]: re.compile(hook["files"])
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
        if "files" in hook
    }
    members = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
        "members"
    ]
    for member in members:
        for directory in ROOT.glob(member):
            path = directory.relative_to(ROOT).as_posix()
            for hook in ("cargo-check", "cargo-clippy-strict", "cargo-test", "cargo-doc"):
                assert hooks[hook].fullmatch(f"{path}/Cargo.toml"), (hook, path)
                assert hooks[hook].fullmatch(f"{path}/src/lib.rs"), (hook, path)
                assert not hooks[hook].search("docs/analysis/unrelated.md"), hook
            if path.startswith("shared/"):
                assert hooks["hexagonal-boundaries"].fullmatch(f"{path}/Cargo.toml"), path
            for hook in (
                "rust-module-discipline",
                "rust-module-cycles",
                "rust-digest-fallibility",
            ):
                assert hooks[hook].search(f"{path}/src/lib.rs"), (hook, path)
            assert bool(hooks["rust-error-shape"].search(f"{path}/src/lib.rs")) == (
                not path.startswith("vendor/")
            ), path
            assert hooks["rust-cargo-toml-hygiene"].search(f"{path}/Cargo.toml"), path


def test_benchmark_build_hook_selects_every_workspace_member() -> None:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    hook = next(
        hook
        for repo in config["repos"]
        if repo["repo"] == "local"
        for hook in repo["hooks"]
        if hook["id"] == "cargo-bench-build"
    )
    pattern = re.compile(hook["files"])
    members = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
        "members"
    ]
    assert hook["stages"] == ["pre-push"]
    assert hook["pass_filenames"] is False
    for path in (
        "Cargo.toml",
        "Cargo.lock",
        ".cargo/config.toml",
        "rust-toolchain.toml",
        "scripts/cargow",
        "scripts/quanta-index-env.sh",
    ):
        assert pattern.fullmatch(path), path
    for member in members:
        for path in (
            f"{member}/Cargo.toml",
            f"{member}/build.rs",
            f"{member}/src/lib.rs",
            f"{member}/src/bin/runner.rs",
            f"{member}/benches/throughput.rs",
        ):
            assert pattern.fullmatch(path), path
    for path in ("docs/analysis/unrelated.md", "tools/ci/tests/test_precommit_scopes.py"):
        assert not pattern.search(path), path


def test_semgrep_keeps_code_scope_but_expands_policy_changes(tmp_path: Path) -> None:
    fake_uv = tmp_path / "uv"
    fake_uv.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n', encoding="utf-8")
    fake_uv.chmod(0o755)
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
        invocation = result.stdout.splitlines()
        assert invocation[:5] == ["run", "--frozen", "--extra", "dev", "semgrep"]
        arguments = invocation[5:]
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
        "crates/quanta-index-searchd/src/lib.rs",
        "tools/ci/tests/test_precommit_scopes.py",
    ]
    assert scan_targets(*code_paths) == code_paths
    full_targets = [".github/workflows"]
    assert scan_targets() == full_targets
    for policy_path in (
        ".semgrepignore",
        "scripts/run-semgrep.sh",
        "tools/ci/semgrep/rules.yml",
    ):
        assert scan_targets(*code_paths, policy_path) == full_targets
        assert scan_targets(*code_paths, f"./{policy_path}") == full_targets
        assert scan_targets(*code_paths, str(ROOT / policy_path)) == full_targets
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
        "cargo-fmt-check": ("rust-fmt", "bash scripts/run-rustfmt.sh check"),
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
    assert "git diff --exit-code docs/reference/sourcegraph-filter-parity.md" in workflow
    recipe = justfile.split("rust-bench-dsl-parity:\n", 1)[1].split("\n\n", 1)[0]
    assert recipe.count("tools/benchmark/sourcegraph_parity.py") == 1
    assert command in recipe


def test_prompt_manager_ci_lints_without_syncing_first() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    steps = workflow["jobs"]["prompt-manager"]["steps"]
    commands = "\n".join(str(step.get("run", "")) for step in steps)
    assert any(step.get("uses", "").startswith("astral-sh/setup-uv@") for step in steps)
    assert "uv run --frozen --extra dev python tools/prompt-manager/pm.py lint" in commands
    assert "uv run --frozen --extra dev bash -c" in commands
    assert "bash scripts/run-tooling-tests.sh" in commands
    assert "tools/prompt-manager/pm.py sync" not in commands
    assert "git diff --exit-code" in commands


def test_tooling_runner_uses_locked_python_and_preserves_exit_status(tmp_path: Path) -> None:
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    runner = scripts / "run-tooling-tests.sh"
    runner.write_bytes((ROOT / "scripts/run-tooling-tests.sh").read_bytes())
    (scripts / "quanta-index-env.sh").write_text(
        'export PYTEST_CACHE_DIR="$PWD/pytest cache"\n', encoding="utf-8"
    )
    fake_uv = tmp_path / "uv"
    fake_uv.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\nexit 23\n', encoding="utf-8")
    fake_uv.chmod(0o755)
    fake_python = tmp_path / "python3"
    fake_python.write_text("#!/bin/sh\nexit 19\n", encoding="utf-8")
    fake_python.chmod(0o755)

    result = subprocess.run(
        ["bash", str(runner), "-k", "proof and not integration"],
        cwd=ROOT,
        env={**os.environ, "PATH": f"{tmp_path}:{os.environ['PATH']}"},
        capture_output=True,
        text=True,
    )

    assert result.returncode == 23, result.stderr
    assert result.stdout.splitlines() == [
        "run",
        "--frozen",
        "--extra",
        "dev",
        "python",
        "-m",
        "pytest",
        "tools",
        "-q",
        "--ignore=tools/ci/tests/test_semgrep_policy.py",
        "--ignore=tools/ci/tests/test_check_rust_fallbacks.py",
        "-o",
        f"cache_dir={tmp_path.resolve() / 'pytest cache'}",
        "-k",
        "proof and not integration",
    ]


@pytest.mark.parametrize("recipe", ["rust-test-pyramid", "verify-rust"])
def test_local_verification_runs_native_frontdoor_contracts(recipe: str) -> None:
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    body = justfile.split(f"\n{recipe}:", 1)[1].split("\n\n", 1)[0]
    assert "@just rust-test-native-frontdoor" in [line.strip() for line in body.splitlines()]


@pytest.mark.parametrize(
    "recipe",
    [
        "rust-test",
        "verify-rust",
        "verify-rust-heavy",
        "rust-careful",
        "rust-tsan",
        "rust-asan",
        "rust-coverage-changed",
        "rust-verify-hellgate-fast",
        "rust-verify-hellgate-broad",
        "rust-test-f15-no-default",
        "rust-test-legacy-crash-compat",
        "rust-test-e2e-fast",
        "rust-test-e2e",
        "rust-test-e2e-all",
        "rust-test-pyramid",
        "rust-verify-quality-relevance",
        "rust-verify-quality-relevance-hash-dev",
        "rust-verify-quality-tail",
        "rust-verify-quality-concurrency",
        "rust-verify-quality-freshness",
        "rust-verify-quality-open-loop",
        "rust-verify-quality-ops",
        "rust-verify-quality-ui",
        "rust-verify-quality-all",
    ],
)
@pytest.mark.parametrize("pin", ["missing", "wrong-digest", "valid"])
def test_legacy_gate_checks_binary_pin_before_build(tmp_path: Path, recipe: str, pin: str) -> None:
    justfile = (ROOT / "Justfile").read_text(encoding="utf-8")
    (tmp_path / "Justfile").write_text(
        justfile.replace('set shell := ["/bin/zsh", "-lc"]', 'set shell := ["/bin/sh", "-c"]', 1),
        encoding="utf-8",
    )
    cargo = tmp_path / "scripts/cargow"
    cargo.parent.mkdir()
    runners = [cargo, tmp_path / "uv", tmp_path / "scripts/run-rustfmt.sh"]
    if recipe in ("verify-rust-heavy", "rust-careful", "rust-tsan", "rust-asan"):
        runners.append(tmp_path / "bash")
    for stub in runners:
        stub.write_text("#!/bin/sh\necho 'Runner reached'\nexit 43\n", encoding="utf-8")
        stub.chmod(0o755)
    contention_check = tmp_path / "tools/ci/timing/check_host_contention.py"
    contention_check.parent.mkdir(parents=True)
    contention_check.write_text("pass\n", encoding="utf-8")
    binary = tmp_path / "old-searchd"
    binary.write_text("#!/bin/sh\nexit 99\n", encoding="utf-8")
    binary.chmod(0o755)
    env = {**os.environ, "PATH": f"{tmp_path}{os.pathsep}{os.environ['PATH']}"}
    env.pop("QUANTA_INDEX_LEGACY_SEARCHD_BIN", None)
    env.pop("QUANTA_INDEX_LEGACY_SEARCHD_SHA256", None)
    if pin != "missing":
        env["QUANTA_INDEX_LEGACY_SEARCHD_BIN"] = str(binary)
        env["QUANTA_INDEX_LEGACY_SEARCHD_SHA256"] = (
            hashlib.sha256(binary.read_bytes()).hexdigest() if pin == "valid" else "0" * 64
        )

    args = ["a" * 40] if recipe == "rust-coverage-changed" else []
    result = subprocess.run(
        ["just", recipe, *args], cwd=tmp_path, env=env, capture_output=True, text=True
    )

    output = result.stdout + result.stderr
    if pin == "valid":
        assert "Runner reached" in output, output
        assert result.returncode == 43, output
    else:
        assert result.returncode == 1, output
        assert "Runner reached" not in output
        assert not (tmp_path / "artifacts").exists()
        expected = (
            "absolute regular executable file"
            if pin == "missing"
            else "differs from the pinned input"
        )
        assert expected in output


def test_native_contract_checkpoint_selects_registered_graph_and_semantic_targets() -> None:
    manifest = tomllib.loads(
        (ROOT / "crates/quanta-index-searchd-runtime/Cargo.toml").read_text(encoding="utf-8")
    )
    assert any(target["name"] == "native_graph_uds" for target in manifest["test"])
    authority = tomllib.loads((ROOT / "tools/ci/test-authority.toml").read_text(encoding="utf-8"))
    targets = {
        "native_graph_uds": "quanta-index-searchd-runtime",
        "native_graph": "quanta-index-kernel",
        "native_semantic_certified_chunks": "quanta-index-kernel",
    }
    registered = {
        (target.get("target"), target["path"]) for target in authority["integration_targets"]
    }
    for name, owner in targets.items():
        assert (name, f"crates/{owner}/tests/{name}.rs") in registered

    result = subprocess.run(
        ["just", "--dry-run", "rust-test-native-index"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    command = shlex.split(result.stderr.splitlines()[-1])
    selected = [command[index + 1] for index, arg in enumerate(command[:-1]) if arg == "--test"]
    for name in targets:
        assert selected.count(name) == 1, name
    selection = command[command.index("-E") + 1]
    assert "package(=quanta-index-kernel)" in selection.split(" | ")
    assert "(package(=quanta-index-searchd-runtime) &" in selection
    assert "binary(=native_graph_uds)" in selection


@pytest.mark.parametrize("omitted", [None, "runtime", "dsl_cursor", "dsl_cursor_event"])
def test_native_frontdoor_checks_each_required_nextest_body(tmp_path: Path, omitted) -> None:
    sdk_cases = [
        "ordinary_sdk_requires_native_selection_before_transport_birth",
        "wire::ordinary_sdk_native_reader_preserves_file_page_hybrid_and_vector_dtos",
        "wire::ordinary_sdk_query_only_has_no_native_write_capability",
        "wire::ordinary_sdk_native_cancel_deadline_and_zero_key_do_not_send",
        "wire::ordinary_sdk_native_rejects_wrong_full_instance_spec_before_query",
        "wire::ordinary_sdk_profile_failure_keeps_acknowledged_session_for_release",
        "wire::ordinary_sdk_native_unknown_session_is_not_reopened_or_queried",
        "wire::ordinary_sdk_native_diagnostics_follow_actual_native_send_outcomes",
        "wire::ordinary_sdk_admitted_reader_keeps_owner_and_checks_captured_role_before_send",
        "wire::ordinary_sdk_admitted_profile_denial_preserves_ack_and_charged_error_until_release",
    ]
    preview = "read_sessions::query::preview"
    dsl_cursor = "read_sessions::dsl::native_dsl_retained_cursor_refuses_another_view_and_keeps_original_rows_v1"
    runtime_cases = [
        "repository_paths::native_uds_repository_paths_page_same_path_and_bind_cursor",
        f"{preview}::native_select_path_combines_path_and_body_matches_after_cold_reopen",
        f"{preview}::native_text_preview_keeps_rank_and_source_coordinates_across_reopen",
        f"{preview}::native_text_preview_keeps_source_byte_boundary_and_refuses_oversized",
        f"{preview}::native_text_preview_work_refusal_keeps_ranked_suffix_after_cold_reopen",
        f"{preview}::native_select_path_rename_and_delete_survive_published_cold_reopen",
        f"{preview}::native_select_path_retained_pages_survive_successor_rename_delete_and_reopen",
        "read_sessions::dsl::native_dsl_twelve_retained_rows_match_independent_gold_v1",
        dsl_cursor,
    ]
    if omitted == "runtime":
        runtime_cases = []
    elif omitted == "dsl_cursor":
        runtime_cases.remove(dsl_cursor)
    inventory = {"test-count": len(sdk_cases) + len(runtime_cases), "rust-suites": {}}
    events = []
    for package, binary, cases in (
        ("quanta-index-sdk", "sdk_native_default", sdk_cases),
        ("quanta-index-searchd-runtime", "native_index_uds", runtime_cases),
    ):
        if not cases:
            continue
        suite = {"crate": package, "test_binary": binary, "kind": "test"}
        inventory["rust-suites"][f"{package}::{binary}"] = {
            "package-name": package,
            "binary-name": binary,
            "kind": "test",
            "status": "listed",
            "testcases": {
                case: {"ignored": False, "filter-match": {"status": "matches"}} for case in cases
            },
        }
        events.append(
            {"type": "suite", "event": "started", "test_count": len(cases), "nextest": suite}
        )
        for case in cases:
            if omitted == "dsl_cursor_event" and case == dsl_cursor:
                continue
            name = f"{package}::{binary}${case}"
            events.extend(
                {"type": "test", "event": event, "name": name} for event in ("started", "ok")
            )
        events.append(
            {
                "type": "suite",
                "event": "ok",
                "passed": len(cases),
                "failed": 0,
                "ignored": 0,
                "nextest": suite,
            }
        )
    payload = "\n".join(json.dumps(event) for event in events)
    run_marker = tmp_path / "nextest started"
    stub = tmp_path / "cargo wrapper"
    stub.write_text(
        "#!/bin/sh\n"
        'case "$4" in\n'
        f"  list) printf '%s\\n' {shlex.quote(json.dumps(inventory))} ;;\n"
        f"  run) : > {shlex.quote(str(run_marker))}; printf '%s\\n' {shlex.quote(payload)} ;;\n"
        "  *) exit 2 ;;\n"
        "esac\n",
        encoding="utf-8",
    )
    stub.chmod(0o700)
    temp_root = tmp_path / "real temp"
    temp_root.mkdir()
    temp_alias = tmp_path / "temp alias"
    temp_alias.symlink_to(temp_root, target_is_directory=True)

    result = subprocess.run(
        ["just", "--set", "cargo", str(stub), "rust-test-native-frontdoor"],
        cwd=ROOT,
        env={**os.environ, "TMPDIR": str(temp_alias)},
        capture_output=True,
        text=True,
    )
    assert list(temp_root.iterdir()) == []
    assert run_marker.is_file() == (omitted not in {"runtime", "dsl_cursor"})

    if omitted is None:
        expected = len(sdk_cases) + len(runtime_cases)
        assert result.returncode == 0, result.stderr
        assert f"Native frontdoor: {expected}/{expected} passed" in result.stdout
    else:
        assert result.returncode == 1
        detail = (
            "nextest suite/test pass counts disagree"
            if omitted == "dsl_cursor_event"
            else "nextest collection differs from the required tests"
        )
        assert detail in result.stderr


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
    assert "python3 tools/ci/lint/check-rust-fallbacks.py" in rust_policy_commands
    assert "python3 tools/ci/tests/test_check_rust_fallbacks.py" in rust_policy_commands


def test_ci_python_jobs_install_only_their_runtime_imports() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    jobs = workflow["jobs"]
    expected = {
        "rust-policy": "python -m pip install 'jsonschema>=4.23.0' 'pyyaml>=6.0.2' 'tree-sitter==0.25.2' 'tree-sitter-language-pack==0.10.0'",
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


def test_msrv_selects_each_all_target_compile_graph_once() -> None:
    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    commands = [
        str(step["run"])
        for step in workflow["jobs"]["rust-msrv"]["steps"]
        if "run" in step and "RUSTUP_TOOLCHAIN=1.92.0" in str(step["run"])
    ]
    expected = [
        "RUSTUP_TOOLCHAIN=1.92.0 ./scripts/cargow --lane msrv-lane test "
        "--workspace --all-targets --all-features --locked --no-run",
        "RUSTUP_TOOLCHAIN=1.92.0 ./scripts/cargow --lane sqlite-oracle-lane check "
        "--manifest-path crates/quanta-index-sqlite-fd-ledger/Cargo.toml "
        "--all-targets --all-features --locked --offline",
    ]
    assert commands == expected
    assert all(
        "Remove rust-toolchain override" != step.get("name")
        for step in workflow["jobs"]["rust-msrv"]["steps"]
    )

    local = subprocess.check_output(
        ["just", "--dry-run", "rust-msrv"], cwd=ROOT, stderr=subprocess.STDOUT, text=True
    )
    assert local.splitlines() == expected


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
