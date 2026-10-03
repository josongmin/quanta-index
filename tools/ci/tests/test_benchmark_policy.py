"""Benchmark control-plane policy: registry, dependency direction, CI bypass."""

from __future__ import annotations

import importlib.util
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
BENCHMARK_DIR = REPO_ROOT / "tools" / "benchmark"
POLICY_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-benchmark-policy.py"

if str(BENCHMARK_DIR) not in sys.path:
    sys.path.insert(0, str(BENCHMARK_DIR))


def _policy_module():
    spec = importlib.util.spec_from_file_location("check_benchmark_policy", POLICY_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _metadata_for(module) -> dict:
    """Cargo-shaped metadata that registers exactly the declared bench targets."""
    registry = module.load_registry(REPO_ROOT / "tools" / "benchmark" / "registry.toml")
    packages = []
    for entry in registry["producers"].values():
        if entry["kind"] != "cargo-bench":
            continue
        packages.append(
            {
                "name": entry["package"],
                "manifest_path": str(REPO_ROOT / "crates" / entry["package"] / "Cargo.toml"),
                "dependencies": [],
                "targets": [{"name": entry["target"], "kind": ["bench"]}],
            }
        )
    return {"packages": packages}


def _write_control_plane(target: Path) -> None:
    destination = target / "tools" / "benchmark"
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "registry.toml").write_text(
        (REPO_ROOT / "tools" / "benchmark" / "registry.toml").read_text(encoding="utf-8"),
        encoding="utf-8",
    )
    (target / "Justfile").write_text(
        (REPO_ROOT / "Justfile").read_text(encoding="utf-8"), encoding="utf-8"
    )


def test_live_control_plane_satisfies_policy() -> None:
    module = _policy_module()
    assert module.check(REPO_ROOT, metadata=_metadata_for(module)) == []


def test_capture_contracts_have_one_local_and_ci_entrypoint() -> None:
    import shlex

    import yaml

    recipes = (REPO_ROOT / "Justfile").read_text(encoding="utf-8")
    body = recipes.split("\nbenchmark-control-contract-local:\n", 1)[1].split("\n\n", 1)[0]
    command = shlex.split(body.strip())
    assert command[:8] == ["uv", "run", "--frozen", "--extra", "dev", "python", "-m", "pytest"]
    from tools.ci.source_closure import PROFILES

    required = {
        path
        for path in PROFILES["benchmark-control-plane"]["paths"]
        if path.startswith("tools/ci/tests/")
    }
    assert required <= set(command[8:]), sorted(required - set(command[8:]))
    for filename in (
        "test_benchmark_profile_capture.py",
        "test_criterion_capture.py",
        "test_recorded_capture.py",
        "test_retrieval_capture.py",
        "test_lexical_capture.py",
        "test_lexical_file_comparison.py",
        "test_lexical_five_product_oracle.py",
        "test_portable_proof.py",
        "test_producer_notifications.py",
        "test_bootstrap_cache.py",
        "test_proof_command_timings.py",
        "test_pair_replay_workspace.py",
        "test_cargo_preparation.py",
    ):
        assert f"tools/ci/tests/{filename}" in command
    assert len(command[8:]) == len(set(command[8:]))
    prep = recipes.split("\nbenchmark-prep-local:\n", 1)[1].split("\n\n", 1)[0]
    invocation = "uv run --frozen --extra dev just benchmark-control-contract-local"
    assert invocation in prep
    assert "pytest" not in prep
    workflow = yaml.safe_load((REPO_ROOT / ".circleci/config.yml").read_text())
    commands = [
        step.get("run", {}).get("command", "")
        for job in workflow["workflows"]["regular"]["jobs"]
        for step in workflow["jobs"][job]["steps"]
        if isinstance(step, dict)
    ]
    assert sum("python -m pytest tools -q" in script for script in commands) == 1
    assert any(
        "python -m pytest tools -q" in step.get("run", {}).get("command", "")
        for step in workflow["jobs"]["verify-python"]["steps"]
        if isinstance(step, dict)
    )
    assert not any(invocation in script for script in commands)


def test_execution_regressions_are_registered_to_the_real_owner_scope():
    import tomllib

    from tools.ci.source_closure import PROFILES

    authority = tomllib.loads((REPO_ROOT / "tools/ci/test-authority.toml").read_text())
    members = authority["python_scopes"]["benchmark-control-capture"]["targets"]
    assert len(members) == len(set(members))
    for filename, owner, retrieval in (
        ("test_producer_notifications.py", "producer_execution.py", False),
        ("test_bootstrap_cache.py", "retrieval/evaluator.py", True),
        ("test_proof_command_timings.py", "retrieval/portable_proof.py", True),
        ("test_pair_replay_workspace.py", "pair_capture.py", False),
        ("test_cargo_preparation.py", "retrieval/portable_proof.py", True),
        ("test_codesearchnet_qrels.py", "retrieval/codesearchnet_qrels.py", True),
        ("test_identifier_robustness_strata.py", "retrieval/identifier_robustness_suite.py", True),
    ):
        path = f"tools/ci/tests/{filename}"
        registered = [entry for entry in authority["python_targets"] if entry["path"] == path]
        assert len(registered) == 1, path
        target = registered[0]
        assert target["id"] in members
        assert target["owner"] == f"tools/benchmark/{owner}"
        assert target["rail"] == "pr-benchmark-control-python"
        profiles = ["benchmark-control-plane", "benchmark-micro", "benchmark-retrieval"]
        if retrieval:
            profiles.append("retrieval")
        for profile in profiles:
            assert path in PROFILES[profile]["paths"], (profile, path)


def test_execution_regression_owners_have_nonempty_live_collection(tmp_path):
    import subprocess

    filenames = (
        "test_producer_notifications.py",
        "test_bootstrap_cache.py",
        "test_proof_command_timings.py",
        "test_pair_replay_workspace.py",
        "test_cargo_preparation.py",
        "test_codesearchnet_qrels.py",
        "test_identifier_robustness_strata.py",
    )
    inventory = tmp_path / "inventory.json"
    result = subprocess.run(
        [
            sys.executable,
            str(REPO_ROOT / "tools/ci/proof_execution_result.py"),
            "collect-pytest",
            "--output",
            str(inventory),
            *(f"tools/ci/tests/{name}" for name in filenames),
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    identities = json.loads(inventory.read_text())["tests"]
    assert len(identities) == len(set(identities))
    for name in filenames:
        prefix = f"tools.ci.tests.{Path(name).stem}."
        assert any(identity.startswith(prefix) for identity in identities), name


def test_inline_benchctl_comment_cannot_hide_direct_producer(tmp_path: Path) -> None:
    module = _policy_module()
    workflow = tmp_path / ".github" / "workflows" / "benchmark.yml"
    workflow.parent.mkdir(parents=True)
    workflow.write_text(
        "jobs:\n  bench:\n    steps:\n      - run: just rust-bench-dsl-warm # benchctl\n",
        encoding="utf-8",
    )
    registry = {"producers": {"warm": {"kind": "just-recipe", "recipe": "rust-bench-dsl-warm"}}}
    assert module.workflow_bypasses(tmp_path, registry)


def test_every_cargo_bench_target_is_registered() -> None:
    module = _policy_module()
    registry = module.load_registry(REPO_ROOT / "tools" / "benchmark" / "registry.toml")
    declared = module.bench_targets(module.cargo_metadata(REPO_ROOT))
    assert declared, "workspace declares no bench targets; the check would be vacuous"
    assert declared == module.registered_bench_targets(registry)


def test_unregistered_bench_target_is_refused(tmp_path: Path) -> None:
    module = _policy_module()
    _write_control_plane(tmp_path)
    metadata = _metadata_for(module)
    metadata["packages"].append(
        {
            "name": "quanta-index-core",
            "manifest_path": str(tmp_path / "crates" / "quanta-index-core" / "Cargo.toml"),
            "dependencies": [],
            "targets": [{"name": "unregistered_bench", "kind": ["bench"]}],
        }
    )
    refusals = module.check(tmp_path, reachability_root=REPO_ROOT, metadata=metadata)
    assert any("unregistered Cargo bench target" in refusal for refusal in refusals)


def test_phantom_registered_bench_target_is_refused(tmp_path: Path) -> None:
    module = _policy_module()
    text = (REPO_ROOT / "tools" / "benchmark" / "registry.toml").read_text(encoding="utf-8")
    mutated = text.replace('target = "pipeline"', 'target = "pipeline_retired"')
    assert mutated != text
    destination = tmp_path / "tools" / "benchmark"
    destination.mkdir(parents=True)
    (destination / "registry.toml").write_text(mutated, encoding="utf-8")
    (tmp_path / "Justfile").write_text(
        (REPO_ROOT / "Justfile").read_text(encoding="utf-8"), encoding="utf-8"
    )
    refusals = module.check(tmp_path, reachability_root=REPO_ROOT, metadata=_metadata_for(module))
    assert any("do not exist" in refusal for refusal in refusals)


def test_production_dependency_on_a_benchmark_package_is_refused() -> None:
    module = _policy_module()
    metadata = {
        "packages": [
            {
                "name": "quanta-index-core",
                "manifest_path": str(REPO_ROOT / "crates" / "quanta-index-core" / "Cargo.toml"),
                "dependencies": [
                    {
                        "name": "quanta-index-bench-protocol",
                        "kind": None,
                        "path": str(REPO_ROOT / "benchmarks" / "bench-protocol"),
                    }
                ],
                "targets": [],
            }
        ]
    }
    inversions = module.dependency_inversions(metadata)
    assert inversions and "quanta-index-bench-protocol" in inversions[0]


def test_dev_dependency_on_a_benchmark_package_is_allowed() -> None:
    module = _policy_module()
    metadata = {
        "packages": [
            {
                "name": "quanta-index-searchd-runtime",
                "manifest_path": str(
                    REPO_ROOT / "crates" / "quanta-index-searchd-runtime" / "Cargo.toml"
                ),
                "dependencies": [
                    {"name": "criterion", "kind": "dev", "path": None},
                    {
                        "name": "quanta-index-bench-protocol",
                        "kind": "dev",
                        "path": str(REPO_ROOT / "benchmarks" / "bench-protocol"),
                    },
                ],
                "targets": [],
            }
        ]
    }
    assert module.dependency_inversions(metadata) == []


def test_direct_ci_producer_invocation_is_refused(tmp_path: Path) -> None:
    module = _policy_module()
    registry = module.load_registry(REPO_ROOT / "tools" / "benchmark" / "registry.toml")
    workflows = tmp_path / ".github" / "workflows"
    workflows.mkdir(parents=True)
    (workflows / "correctness.yml").write_text(
        "jobs:\n  x:\n    steps:\n      - run: just rust-bench-dsl-warm\n",
        encoding="utf-8",
    )
    bypasses = module.workflow_bypasses(tmp_path, registry)
    assert any("rust-bench-dsl-warm" in entry for entry in bypasses)


def test_registry_cli_ci_invocation_is_allowed(tmp_path: Path) -> None:
    module = _policy_module()
    registry = module.load_registry(REPO_ROOT / "tools" / "benchmark" / "registry.toml")
    workflows = tmp_path / ".github" / "workflows"
    workflows.mkdir(parents=True)
    (workflows / "correctness.yml").write_text(
        "jobs:\n  x:\n    steps:\n"
        "      - run: python3 tools/benchmark/benchctl.py run dsl-authority\n",
        encoding="utf-8",
    )
    assert module.workflow_bypasses(tmp_path, registry) == []


def test_legacy_authority_family_is_refused(tmp_path: Path) -> None:
    module = _policy_module()
    text = (REPO_ROOT / "tools" / "benchmark" / "registry.toml").read_text(encoding="utf-8")
    mutated, count = re.subn(
        r'\nauthority = "registry"\n', '\nauthority = "legacy"\n', text, count=1
    )
    assert count == 1, "fixture did not mutate a family authority"
    destination = tmp_path / "tools" / "benchmark"
    destination.mkdir(parents=True)
    (destination / "registry.toml").write_text(mutated, encoding="utf-8")
    (tmp_path / "Justfile").write_text(
        (REPO_ROOT / "Justfile").read_text(encoding="utf-8"), encoding="utf-8"
    )
    refusals = module.check(tmp_path, reachability_root=REPO_ROOT, metadata=_metadata_for(module))
    assert any("legacy authority" in refusal for refusal in refusals)


def test_dependency_direction_of_the_real_workspace_is_recorded() -> None:
    """The live workspace must not invert the product/benchmark dependency edge."""
    module = _policy_module()
    metadata = module.cargo_metadata(REPO_ROOT)
    assert module.dependency_inversions(metadata) == []
    assert json.dumps(metadata)[:1] == "{"
