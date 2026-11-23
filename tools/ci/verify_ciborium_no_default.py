"""Verify the standalone registry Ciborium consumer has no runtime std feature."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "tools/ci/ciborium-no-default-consumer/Cargo.toml"
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
RUNTIME_PACKAGES = frozenset(
    {"ciborium", "ciborium-io", "ciborium-ll", "half", "serde", "serde_core", "zerocopy"}
)


def validate(metadata: dict[str, Any]) -> None:
    packages = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages, list) or not isinstance(resolve, dict):
        raise ValueError("Cargo metadata is missing packages or resolved features")
    nodes = resolve.get("nodes")
    if not isinstance(nodes, list):
        raise ValueError("Cargo metadata is missing resolved nodes")
    features_by_id = {node["id"]: node["features"] for node in nodes}
    by_name: dict[str, list[dict[str, Any]]] = {}
    for package in packages:
        by_name.setdefault(package["name"], []).append(package)
    for name in RUNTIME_PACKAGES:
        selected = by_name.get(name, [])
        if len(selected) != 1 or selected[0].get("source") != REGISTRY:
            raise ValueError(f"{name} must resolve once from crates.io")
        features = features_by_id.get(selected[0]["id"])
        if not isinstance(features, list) or "std" in features:
            raise ValueError(f"{name} unexpectedly enables std or lacks a resolved node")
    if by_name["ciborium"][0]["version"] != "0.2.2":
        raise ValueError("ciborium version must be 0.2.2")
    roots = by_name.get("ciborium-no-default-consumer", [])
    if len(roots) != 1:
        raise ValueError("standalone no-default consumer is missing or ambiguous")
    dependencies = [dep for dep in roots[0].get("dependencies", []) if dep["name"] == "ciborium"]
    if len(dependencies) != 1 or dependencies[0].get("uses_default_features") is not False:
        raise ValueError("consumer must disable Ciborium default features")


def main() -> None:
    environment = dict(os.environ, QUANTA_INDEX_TARGET_GC="0")
    result = subprocess.run(
        [
            str(ROOT / "scripts/cargow"),
            "metadata",
            "--manifest-path",
            str(MANIFEST),
            "--locked",
            "--offline",
            "--format-version",
            "1",
        ],
        cwd=ROOT,
        env=environment,
        check=True,
        capture_output=True,
        text=True,
    )
    validate(json.loads(result.stdout))
    print("registry Ciborium runtime graph: no std features")


if __name__ == "__main__":
    main()
