#!/usr/bin/env python3
"""Refuse an unregistered benchmark producer, a dependency inversion or a CI bypass.

This is the machine-checkable half of the benchmark control-plane contract:

1. `tools/benchmark/registry.toml` is valid and every producer/validator/scorer
   it names exists in this checkout;
2. every Cargo bench target in the workspace is registered (an unregistered
   producer fails policy);
3. no production `crates/*` package takes a normal dependency on a
   `benchmarks/*` benchmark-only package;
4. no CI workflow invokes a registered timing producer or comparator directly
   once its family has cut over to the registry authority;
5. no family is left in an ambiguous `legacy` authority state.

Exit codes: 0 clean, 1 refused, 2 usage.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
BENCHMARK_DIR = REPO_ROOT / "tools" / "benchmark"
if str(BENCHMARK_DIR) not in sys.path:
    sys.path.insert(0, str(BENCHMARK_DIR))

from registry import RegistryError, load_registry, registry_digest  # noqa: E402

PRODUCTION_ROOT = "crates/"
BENCHMARK_ROOT = "benchmarks/"

#: Workflows may name a registered producer only through the registry CLI.
JUST_INVOCATION = re.compile(r"\bjust\s+([a-z0-9][a-z0-9-]*)")
SCORER_INVOCATION = re.compile(
    r"python3?\s+tools/benchmark/(?:compare_dsl_bench|quality_integration_summary)\.py"
)


class PolicyRefusal(RuntimeError):
    """The benchmark control plane policy was violated."""


def cargo_metadata(repo_root: Path) -> dict:
    """Read-only Cargo graph for producer and dependency-direction checks."""
    try:
        raw = subprocess.check_output(
            [
                str(repo_root / "scripts" / "cargow"),
                "--lane",
                "metadata-lane",
                "metadata",
                "--format-version",
                "1",
                "--no-deps",
                "--locked",
            ],
            cwd=repo_root,
            stderr=subprocess.STDOUT,
        )
    except subprocess.CalledProcessError as error:
        raise PolicyRefusal("cargo metadata failed while checking policy") from error
    payload = json.loads(raw)
    if not isinstance(payload, dict):
        raise PolicyRefusal("cargo metadata must return an object")
    return payload


def bench_targets(metadata: dict) -> set[tuple[str, str]]:
    """`(package name, bench target name)` pairs declared by the workspace."""
    targets: set[tuple[str, str]] = set()
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise PolicyRefusal("cargo metadata omitted packages")
    for package in packages:
        if not isinstance(package, dict):
            raise PolicyRefusal("cargo metadata package entry is not an object")
        name = package.get("name")
        for target in package.get("targets") or []:
            if not isinstance(target, dict):
                continue
            if "bench" in (target.get("kind") or []):
                targets.add((str(name), str(target.get("name"))))
    return targets


def registered_bench_targets(registry: dict) -> set[tuple[str, str]]:
    pairs: set[tuple[str, str]] = set()
    for entry in registry["producers"].values():
        if entry["kind"] == "cargo-bench":
            pairs.add((entry["package"], entry["target"]))
    return pairs


def dependency_inversions(metadata: dict) -> list[str]:
    """Production packages that depend on a benchmark-only package."""
    inversions: list[str] = []
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise PolicyRefusal("cargo metadata omitted packages")
    for package in packages:
        if not isinstance(package, dict):
            continue
        manifest = str(package.get("manifest_path") or "")
        relative = manifest.split(f"/{PRODUCTION_ROOT}", 1)
        if len(relative) != 2:
            continue
        for dependency in package.get("dependencies") or []:
            if not isinstance(dependency, dict):
                continue
            kind = dependency.get("kind")
            if kind is not None:
                # dev/build dependencies are allowed; only normal edges ship.
                continue
            path = str(dependency.get("path") or "")
            if BENCHMARK_ROOT in path:
                inversions.append(
                    f"{package.get('name')} -> {dependency.get('name')} ({path})"
                )
    return inversions


def workflow_bypasses(repo_root: Path, registry: dict) -> list[str]:
    """Direct CI calls to producers/comparators that live behind the CLI."""
    import yaml

    protected_recipes = {
        entry["recipe"]
        for entry in registry["producers"].values()
        if entry["kind"] == "just-recipe"
    }
    bypasses: list[str] = []
    workflows = repo_root / ".github" / "workflows"
    if not workflows.is_dir():
        return bypasses
    for path in sorted(workflows.glob("*.yml")):
        try:
            workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
        except (OSError, yaml.YAMLError) as exc:
            bypasses.append(f"{path.name}: cannot parse workflow: {exc}")
            continue
        if not isinstance(workflow, dict) or not isinstance(workflow.get("jobs"), dict):
            bypasses.append(f"{path.name}: workflow has no jobs mapping")
            continue
        for job_id, job in workflow["jobs"].items():
            if not isinstance(job, dict):
                continue
            for step in job.get("steps", []):
                if not isinstance(step, dict) or not isinstance(step.get("run"), str):
                    continue
                script = re.sub(r"\\\r?\n[ \t]*", " ", step["run"])
                for line in script.splitlines():
                    executable = line.split("#", 1)[0].strip()
                    if not executable:
                        continue
                    for match in JUST_INVOCATION.finditer(executable):
                        if match.group(1) in protected_recipes:
                            bypasses.append(f"{path.name}:{job_id}: {executable}")
                    if SCORER_INVOCATION.search(executable):
                        bypasses.append(f"{path.name}:{job_id}: {executable}")
    return bypasses


def check(
    repo_root: Path,
    *,
    metadata: dict | None = None,
    reachability_root: Path | None = None,
) -> list[str]:
    """Return every policy refusal for the current control plane.

    `repo_root` selects the checkout whose Cargo graph and workflows are
    checked; `reachability_root` selects the checkout that must contain the
    named producers (the checkout this linter ships with by default).
    """
    refusals: list[str] = []
    try:
        registry = load_registry(
            repo_root / "tools" / "benchmark" / "registry.toml",
            repo_root=reachability_root or repo_root,
        )
    except RegistryError as error:
        return [f"registry is invalid: {error}"]

    legacy = sorted(
        name
        for name, family in registry["families"].items()
        if family["authority"] == "legacy"
    )
    if legacy:
        refusals.append(f"families still in an ambiguous legacy authority: {', '.join(legacy)}")

    graph = metadata if metadata is not None else cargo_metadata(repo_root)
    declared = bench_targets(graph)
    registered = registered_bench_targets(registry)
    unregistered = sorted(declared - registered)
    if unregistered:
        refusals.append(
            "unregistered Cargo bench target(s): "
            + ", ".join(f"{package}:{target}" for package, target in unregistered)
        )
    phantom = sorted(registered - declared)
    if phantom:
        refusals.append(
            "registry names Cargo bench target(s) that do not exist: "
            + ", ".join(f"{package}:{target}" for package, target in phantom)
        )

    inversions = dependency_inversions(graph)
    if inversions:
        refusals.append("production crates depend on benchmark-only packages: " + "; ".join(inversions))

    bypasses = workflow_bypasses(repo_root, registry)
    if bypasses:
        refusals.append("CI invokes a registered producer/comparator directly: " + "; ".join(bypasses))

    return refusals


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    parser.add_argument(
        "--print-registry-digest",
        action="store_true",
        help="print the canonical registry digest after a clean check",
    )
    args = parser.parse_args(argv)
    repo_root = args.repo_root.resolve()
    if not (repo_root / "tools" / "benchmark" / "registry.toml").is_file():
        print(f"ERROR: no benchmark registry under {repo_root}", file=sys.stderr)
        return 2
    try:
        refusals = check(repo_root, reachability_root=REPO_ROOT)
    except PolicyRefusal as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    if refusals:
        for refusal in refusals:
            print(f"REFUSED: {refusal}", file=sys.stderr)
        return 1
    if args.print_registry_digest:
        registry = load_registry(repo_root / "tools" / "benchmark" / "registry.toml")
        print(f"registry digest: {registry_digest(registry)}")
    print("benchmark control-plane policy ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
