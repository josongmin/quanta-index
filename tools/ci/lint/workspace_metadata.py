"""Cargo-resolved workspace package and library target authority for CI ratchets."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

LIBRARY_TARGET_KINDS = ("lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro")


def workspace_members(metadata: object) -> dict[str, dict]:
    """Use Cargo's resolved members, including globs, exclusions and path deps."""
    if not isinstance(metadata, dict):
        raise RuntimeError("cargo metadata root is not an object")
    packages = metadata.get("packages")
    members = metadata.get("workspace_members")
    if not isinstance(packages, list) or not isinstance(members, list) or not members:
        raise RuntimeError("cargo metadata has no package or workspace member list")
    resolve = metadata.get("resolve")
    nodes = resolve.get("nodes") if isinstance(resolve, dict) else None
    if not isinstance(nodes, list) or not nodes:
        raise RuntimeError("cargo metadata has no resolved dependency graph")
    by_id: dict[str, str] = {}
    for package in packages:
        if not isinstance(package, dict):
            raise RuntimeError("cargo metadata has an invalid package")
        package_id = package.get("id")
        name = package.get("name")
        if (
            not isinstance(package_id, str)
            or not package_id
            or not isinstance(name, str)
            or not name
        ):
            raise RuntimeError("cargo metadata has an invalid package identity")
        if package_id in by_id:
            raise RuntimeError(f"cargo metadata repeats package identity: {package_id}")
        by_id[package_id] = name
    if any(not isinstance(member, str) or member not in by_id for member in members):
        raise RuntimeError("cargo metadata workspace member has no package")
    node_ids: set[str] = set()
    dependency_ids: set[str] = set()
    for node in nodes:
        if (
            not isinstance(node, dict)
            or not isinstance(node.get("id"), str)
            or not isinstance(node.get("dependencies"), list)
            or any(not isinstance(dep, str) for dep in node["dependencies"])
        ):
            raise RuntimeError("cargo metadata has an invalid dependency node")
        if (
            node["id"] in node_ids
            or node["id"] not in by_id
            or any(dep not in by_id for dep in node["dependencies"])
        ):
            raise RuntimeError("cargo metadata dependency node refers to a missing package")
        node_ids.add(node["id"])
        dependency_ids.update(node["dependencies"])
    if any(member not in node_ids for member in members):
        raise RuntimeError("cargo metadata workspace member has no dependency node")
    if missing_nodes := dependency_ids - node_ids:
        raise RuntimeError(
            f"cargo metadata dependencies lack resolve nodes: {sorted(missing_nodes)}"
        )
    packages_by_id = {package["id"]: package for package in packages}
    selected = {by_id[member]: packages_by_id[member] for member in members}
    if len(selected) != len(members):
        raise RuntimeError("cargo metadata repeats a workspace package")
    return selected


def library_target_name(package: str, metadata: dict) -> str:
    targets = metadata.get("targets")
    if not isinstance(targets, list):
        raise RuntimeError(f"cargo metadata has no targets for {package}")
    libraries = [
        target.get("name")
        for target in targets
        if isinstance(target, dict)
        and isinstance(target.get("kind"), list)
        and any(kind in target["kind"] for kind in LIBRARY_TARGET_KINDS)
    ]
    if len(libraries) != 1 or not isinstance(libraries[0], str) or not libraries[0]:
        raise RuntimeError(f"cargo metadata has no unique library target for {package}")
    return libraries[0]


def load_workspace_members(root: Path, env: dict[str, str]) -> dict[str, dict]:
    result = subprocess.run(
        [str(root / "scripts" / "cargow"), "metadata", "--locked", "--format-version", "1"],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(f"workspace dependency resolution failed: {result.stderr}")
    try:
        metadata = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"cargo metadata is not valid JSON: {error}") from error
    return workspace_members(metadata)
