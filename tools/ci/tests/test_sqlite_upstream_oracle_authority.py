"""Keep the standalone SQLite source and active CI oracle binding explicit."""

from __future__ import annotations

import copy
from pathlib import Path

import pytest
import tomllib
import yaml

from tools.ci.verify_sqlite_upstream_oracle import (
    validate_lock,
    validate_metadata,
    validate_product_lock,
    validate_product_metadata,
)

ROOT = Path(__file__).resolve().parents[3]
LOCK = ROOT / "tools/sqlite-upstream-oracle/Cargo.lock"
PRODUCT_LOCK = ROOT / "crates/quanta-index-sqlite-unix-syscall/Cargo.lock"


def test_oracle_lock_selects_both_registry_sources() -> None:
    validate_lock(tomllib.loads(LOCK.read_text(encoding="utf-8")))


def test_product_hook_lock_rejects_substituted_sqlite() -> None:
    lock = tomllib.loads(PRODUCT_LOCK.read_text(encoding="utf-8"))
    validate_product_lock(lock)
    package = next(package for package in lock["package"] if package["name"] == "libsqlite3-sys")
    package["source"] = "git+https://example.invalid/fork"
    with pytest.raises(ValueError, match="must use registry"):
        validate_product_lock(lock)


def test_locked_sqlite_policy_rejects_previous_version() -> None:
    oracle = tomllib.loads(LOCK.read_text(encoding="utf-8"))
    product = tomllib.loads(PRODUCT_LOCK.read_text(encoding="utf-8"))
    for lock, validate in ((oracle, validate_lock), (product, validate_product_lock)):
        package = next(
            package for package in lock["package"] if package["name"] == "libsqlite3-sys"
        )
        package["version"] = "0.38.2"
        with pytest.raises(ValueError, match="0.39.1"):
            validate(lock)


def test_product_hook_metadata_rejects_local_patch() -> None:
    metadata = {
        "packages": [
            {"id": "root", "name": "quanta-index-sqlite-unix-syscall"},
            {
                "id": "sqlite",
                "name": "libsqlite3-sys",
                "version": "0.39.1",
                "source": "registry+https://github.com/rust-lang/crates.io-index",
            },
        ],
        "resolve": {"root": "root", "nodes": [{"id": "root"}, {"id": "sqlite"}]},
    }
    validate_product_metadata(metadata)
    previous = copy.deepcopy(metadata)
    previous["packages"][1]["version"] = "0.38.2"
    with pytest.raises(ValueError, match="0.39.1"):
        validate_product_metadata(previous)
    metadata["packages"][1]["source"] = None
    with pytest.raises(ValueError, match="must resolve registry"):
        validate_product_metadata(metadata)


@pytest.mark.parametrize("name", ["rusqlite", "libsqlite3-sys"])
def test_oracle_lock_rejects_substituted_source(name: str) -> None:
    lock = tomllib.loads(LOCK.read_text(encoding="utf-8"))
    package = next(package for package in lock["package"] if package["name"] == name)
    package["source"] = "git+https://example.invalid/fork"
    with pytest.raises(ValueError, match="must be registry"):
        validate_lock(lock)


@pytest.mark.parametrize("name", ["rusqlite", "libsqlite3-sys"])
def test_oracle_metadata_rejects_local_patch(name: str) -> None:
    packages = [
        {"id": "root", "name": "quanta-index-sqlite-upstream-oracle"},
        {
            "id": "rusqlite",
            "name": "rusqlite",
            "version": "0.41.0",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
        },
        {
            "id": "sqlite",
            "name": "libsqlite3-sys",
            "version": "0.39.1",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
        },
    ]
    metadata = {
        "packages": packages,
        "resolve": {"root": "root", "nodes": [{"id": item["id"]} for item in packages]},
    }
    substituted = copy.deepcopy(metadata)
    next(package for package in substituted["packages"] if package["name"] == name)["source"] = None
    validate_metadata(metadata)
    with pytest.raises(ValueError, match="must resolve from registry"):
        validate_metadata(substituted)


def test_active_circleci_fetches_validates_and_runs_oracle() -> None:
    config = yaml.safe_load((ROOT / ".circleci/config.yml").read_text(encoding="utf-8"))
    steps = config["jobs"]["verify-rust-tests"]["steps"]
    selected = [
        step["run"]["command"]
        for step in steps
        if isinstance(step, dict)
        and isinstance(step.get("run"), dict)
        and step["run"].get("name") == "upstream SQLite WAL oracle"
    ]
    assert len(selected) == 1
    script = selected[0]
    required = [
        "./scripts/cargow --lane sqlite-oracle-lane fetch --manifest-path tools/sqlite-upstream-oracle/Cargo.toml --locked",
        "uv run --frozen --extra dev python tools/ci/verify_sqlite_upstream_oracle.py",
        "just rust-test-sqlite-upstream-oracle",
    ]
    assert all(line in script.splitlines() for line in required)
    assert [script.index(line) for line in required] == sorted(
        script.index(line) for line in required
    )
    recipe = (ROOT / "Justfile").read_text(encoding="utf-8")
    assert 'rust-test-sqlite-upstream-oracle lane="sqlite-oracle-lane":' in recipe
    assert (
        "test --manifest-path tools/sqlite-upstream-oracle/Cargo.toml --locked --offline" in recipe
    )
    assert (
        "test --manifest-path crates/quanta-index-sqlite-unix-syscall/Cargo.toml --locked --offline"
        in recipe
    )
