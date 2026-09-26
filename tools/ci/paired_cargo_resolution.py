"""Reject cross-repository proofs resolving Quanta crates from another checkout.

Input is Cargo metadata with a resolve graph, not declared dependency paths.
This guard is execution preflight; its JSON is not a qualification receipt.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 compatibility, matching proof tooling.
    import tomli as tomllib


REQUIRED_QUANTA_PACKAGES = frozenset(
    {"quanta-index-contract", "quanta-index-ipc", "quanta-index-sdk"}
)


def _unique_json(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _digest(path: Path) -> str:
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def _path(value: Any, label: str) -> Path:
    if not isinstance(value, str) or not value or not Path(value).is_absolute():
        raise ValueError(f"{label} must be an absolute path")
    return Path(value).resolve(strict=True)


def validate_resolution(
    metadata: Any, *, quanta_root: Path, paired_root: Path, consumer: str
) -> dict[str, Any]:
    """Return stable relative identities only after validating actual resolution."""

    quanta_root = quanta_root.resolve(strict=True)
    paired_root = paired_root.resolve(strict=True)
    if (
        not isinstance(metadata, dict)
        or type(metadata.get("version")) is not int
        or metadata["version"] != 1
    ):
        raise ValueError("Cargo metadata format version 1 is required")
    workspace = _path(metadata.get("workspace_root"), "workspace root")
    expected_workspace = paired_root / "packages/analysis/quanta-v2"
    if not workspace.is_relative_to(paired_root):
        raise ValueError("Cargo workspace escapes the paired checkout")
    if workspace != expected_workspace.resolve(strict=True):
        raise ValueError("Cargo resolver belongs to another paired workspace")
    lock = workspace / "Cargo.lock"
    lock = lock.resolve(strict=True)
    if lock.parent != workspace:
        raise ValueError("Cargo resolver lock escapes its workspace")

    packages_raw = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages_raw, list) or not packages_raw:
        raise ValueError("Cargo package inventory is missing")
    if not isinstance(resolve, dict) or not isinstance(resolve.get("nodes"), list):
        raise ValueError("Cargo resolve graph is required (do not use --no-deps)")
    packages: dict[str, dict[str, Any]] = {}
    for package in packages_raw:
        if not isinstance(package, dict) or not isinstance(package.get("id"), str):
            raise ValueError("malformed Cargo package")
        identity = package["id"]
        if identity in packages:
            raise ValueError("duplicate Cargo package identity")
        packages[identity] = package
    nodes: dict[str, dict[str, Any]] = {}
    for node in resolve["nodes"]:
        if not isinstance(node, dict) or node.get("id") not in packages:
            raise ValueError("unknown Cargo resolve node")
        if node["id"] in nodes:
            raise ValueError("duplicate Cargo resolve node")
        dependencies = node.get("dependencies")
        if (
            not isinstance(dependencies, list)
            or any(
                not isinstance(identity, str) or identity not in packages
                for identity in dependencies
            )
            or len(set(dependencies)) != len(dependencies)
        ):
            raise ValueError("malformed Cargo dependency edge")
        nodes[node["id"]] = node
    roots = [identity for identity, package in packages.items() if package.get("name") == consumer]
    if len(roots) != 1:
        raise ValueError("selected consumer must resolve exactly once")
    consumer_manifest = _path(packages[roots[0]].get("manifest_path"), "consumer manifest")
    if not consumer_manifest.is_relative_to(workspace):
        raise ValueError("selected consumer escapes the paired workspace")
    expected_consumer = workspace / "crates" / consumer / "Cargo.toml"
    if consumer_manifest != expected_consumer.resolve(strict=True):
        raise ValueError("selected consumer comes from another checkout")
    reachable: set[str] = set()
    pending = roots.copy()
    while pending:
        identity = pending.pop()
        if identity in reachable:
            continue
        if identity not in nodes:
            raise ValueError("reachable Cargo dependency lacks a resolve node")
        reachable.add(identity)
        pending.extend(nodes[identity]["dependencies"])
    required_feature = {
        "quanta-runtime": "index-sdk-ingress",
        "quanta-runtime-retrieval-kernel": "index-sdk-ingress-surface",
    }.get(consumer)
    consumer_features = nodes[roots[0]].get("features")
    if (
        required_feature is None
        or not isinstance(consumer_features, list)
        or any(not isinstance(feature, str) for feature in consumer_features)
        or len(set(consumer_features)) != len(consumer_features)
        or required_feature not in consumer_features
    ):
        raise ValueError("selected consumer feature profile is not enabled")

    # Derive the expected manifest owners from this workspace, not from a
    # second hard-coded package-to-path registry or a sibling-directory guess.
    workspace_manifest = quanta_root / "Cargo.toml"
    workspace_config = tomllib.loads(workspace_manifest.read_text())
    expected: dict[str, Path] = {}
    for member in workspace_config["workspace"]["members"]:
        manifest = (quanta_root / member / "Cargo.toml").resolve(strict=True)
        if not manifest.is_relative_to(quanta_root):
            raise ValueError("Quanta workspace member escapes the expected checkout")
        config = tomllib.loads(manifest.read_text())
        name = config["package"]["name"]
        if name in expected:
            raise ValueError("duplicate expected Quanta workspace package")
        expected[name] = manifest
    resolved_packages: list[dict[str, Any]] = []
    seen_names: set[str] = set()
    for identity in sorted(reachable):
        package = packages[identity]
        name = package.get("name")
        if not isinstance(name, str):
            raise ValueError("Cargo package name is missing")
        if not name.startswith("quanta-index-"):
            continue
        if name in seen_names or name not in expected:
            raise ValueError(f"ambiguous or unknown resolved Quanta package: {name}")
        seen_names.add(name)
        manifest = _path(package.get("manifest_path"), "Quanta dependency manifest")
        if "source" not in package or package["source"] is not None or manifest != expected[name]:
            raise ValueError(f"resolved Quanta dependency belongs to another source: {name}")
        if not isinstance(package.get("version"), str) or not package["version"]:
            raise ValueError("Cargo package version is missing")
        features = nodes[identity].get("features")
        if (
            not isinstance(features, list)
            or any(not isinstance(feature, str) for feature in features)
            or len(set(features)) != len(features)
        ):
            raise ValueError("malformed resolved Quanta features")
        resolved_packages.append(
            {
                "name": name,
                "version": package["version"],
                "manifest": manifest.relative_to(quanta_root).as_posix(),
                "manifest_sha256": _digest(manifest),
                "features": sorted(features),
            }
        )
    if not REQUIRED_QUANTA_PACKAGES.issubset(seen_names):
        raise ValueError(
            "selected feature profile does not resolve all required Quanta dependencies"
        )
    return {
        "version": 1,
        "consumer": consumer,
        "consumer_features": sorted(consumer_features),
        "workspace": workspace.relative_to(paired_root).as_posix(),
        "dependency_lock": {
            "path": lock.relative_to(paired_root).as_posix(),
            "sha256": _digest(lock),
        },
        "quanta_workspace_manifest_sha256": _digest(workspace_manifest),
        "packages": sorted(resolved_packages, key=lambda item: item["name"]),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--quanta-root", type=Path, required=True)
    parser.add_argument("--paired-root", type=Path, required=True)
    parser.add_argument(
        "--consumer", choices=["quanta-runtime", "quanta-runtime-retrieval-kernel"], required=True
    )
    args = parser.parse_args()
    try:
        metadata = json.load(sys.stdin, object_pairs_hook=_unique_json)
        result = validate_resolution(
            metadata,
            quanta_root=args.quanta_root,
            paired_root=args.paired_root,
            consumer=args.consumer,
        )
    except (ValueError, OSError, KeyError, TypeError) as error:
        print(f"paired Cargo dependency resolution refused: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
