"""Reject a patched or substituted SQLite source in the standalone oracle."""

from __future__ import annotations

import json
import os
import re
import subprocess
from pathlib import Path
from typing import Any

import tomllib

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "tools/sqlite-upstream-oracle/Cargo.toml"
LOCK = MANIFEST.with_name("Cargo.lock")
PRODUCT_MANIFEST = ROOT / "crates/quanta-index-sqlite-unix-syscall/Cargo.toml"
PRODUCT_LOCK = PRODUCT_MANIFEST.with_name("Cargo.lock")
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
EXPECTED = {"rusqlite": "0.41.0", "libsqlite3-sys": "0.39.1"}


def _one_package(packages: list[dict[str, Any]], name: str) -> dict[str, Any]:
    selected = [package for package in packages if package.get("name") == name]
    if len(selected) != 1:
        raise ValueError(f"{name} must resolve exactly once")
    return selected[0]


def validate_lock(lock: dict[str, Any]) -> None:
    packages = lock.get("package")
    if not isinstance(packages, list):
        raise ValueError("oracle Cargo.lock has no package list")
    for name, version in EXPECTED.items():
        package = _one_package(packages, name)
        if package.get("version") != version or package.get("source") != REGISTRY:
            raise ValueError(f"{name} must be registry version {version} in oracle lock")
        if not re.fullmatch(r"[0-9a-f]{64}", package.get("checksum", "")):
            raise ValueError(f"{name} must have a registry checksum")


def validate_metadata(metadata: dict[str, Any]) -> None:
    packages = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages, list) or not isinstance(resolve, dict):
        raise ValueError("oracle metadata has no resolved package graph")
    nodes = resolve.get("nodes")
    if not isinstance(nodes, list):
        raise ValueError("oracle metadata has no resolved nodes")
    node_ids = {node.get("id") for node in nodes}
    for name, version in EXPECTED.items():
        package = _one_package(packages, name)
        if package.get("version") != version or package.get("source") != REGISTRY:
            raise ValueError(f"{name} must resolve from registry version {version}")
        if package.get("id") not in node_ids:
            raise ValueError(f"{name} is missing from the resolved graph")
    root = _one_package(packages, "quanta-index-sqlite-upstream-oracle")
    if resolve.get("root") != root.get("id"):
        raise ValueError("oracle workspace root differs from the standalone package")


def validate_product_lock(lock: dict[str, Any]) -> None:
    packages = lock.get("package")
    if not isinstance(packages, list):
        raise ValueError("product Unix hook Cargo.lock has no package list")
    package = _one_package(packages, "libsqlite3-sys")
    if package.get("version") != EXPECTED["libsqlite3-sys"] or package.get("source") != REGISTRY:
        raise ValueError(f"product Unix hook must use registry libsqlite3-sys {EXPECTED['libsqlite3-sys']}")
    if not re.fullmatch(r"[0-9a-f]{64}", package.get("checksum", "")):
        raise ValueError("product Unix hook registry checksum missing")


def validate_product_metadata(metadata: dict[str, Any]) -> None:
    packages = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages, list) or not isinstance(resolve, dict):
        raise ValueError("product Unix hook metadata has no package graph")
    package = _one_package(packages, "libsqlite3-sys")
    if package.get("version") != EXPECTED["libsqlite3-sys"] or package.get("source") != REGISTRY:
        raise ValueError(f"product Unix hook must resolve registry libsqlite3-sys {EXPECTED['libsqlite3-sys']}")
    root = _one_package(packages, "quanta-index-sqlite-unix-syscall")
    if resolve.get("root") != root.get("id"):
        raise ValueError("product Unix hook root differs from its package")
    nodes = resolve.get("nodes")
    if not isinstance(nodes, list) or package.get("id") not in {node.get("id") for node in nodes}:
        raise ValueError("product Unix hook SQLite package missing from resolved graph")


def _metadata(manifest: Path) -> dict[str, Any]:
    result = subprocess.run(
        [
            str(ROOT / "scripts/cargow"),
            "metadata",
            "--manifest-path",
            str(manifest),
            "--locked",
            "--offline",
            "--format-version",
            "1",
        ],
        cwd=ROOT,
        env={**os.environ, "QUANTA_INDEX_TARGET_GC": "0"},
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def main() -> None:
    manifest = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    if "workspace" not in manifest or manifest.get("patch"):
        raise ValueError("oracle must be a standalone workspace without Cargo patches")
    dependency = manifest.get("dev-dependencies", {}).get("rusqlite", {})
    if dependency.get("version") != f"={EXPECTED['rusqlite']}" or "bundled" not in dependency.get("features", []):
        raise ValueError(f"oracle must use bundled registry rusqlite {EXPECTED['rusqlite']}")
    validate_lock(tomllib.loads(LOCK.read_text(encoding="utf-8")))
    validate_metadata(_metadata(MANIFEST))
    product = tomllib.loads(PRODUCT_MANIFEST.read_text(encoding="utf-8"))
    if "workspace" not in product or product.get("patch"):
        raise ValueError("product Unix hook must be a standalone unpatched workspace")
    sqlite = product.get("dependencies", {}).get("libsqlite3-sys", {})
    if sqlite.get("version") != f"={EXPECTED['libsqlite3-sys']}" or "bundled" not in sqlite.get("features", []):
        raise ValueError(f"product Unix hook must pin bundled registry SQLite {EXPECTED['libsqlite3-sys']}")
    validate_product_lock(tomllib.loads(PRODUCT_LOCK.read_text(encoding="utf-8")))
    validate_product_metadata(_metadata(PRODUCT_MANIFEST))
    print(f"SQLite oracle and product Unix hook: registry libsqlite3-sys {EXPECTED['libsqlite3-sys']}")


if __name__ == "__main__":
    main()
