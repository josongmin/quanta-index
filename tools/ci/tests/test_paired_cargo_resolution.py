"""Independent malformed/wrong-checkout counterexamples for the resolver guard."""

from __future__ import annotations

import copy
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

MODULE_PATH = Path(__file__).resolve().parents[1] / "paired_cargo_resolution.py"
SPEC = importlib.util.spec_from_file_location("paired_cargo_resolution", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


@pytest.fixture
def resolution(tmp_path):
    quanta = tmp_path / "quanta"
    paired = tmp_path / "paired"
    workspace = paired / "packages/analysis/quanta-v2"
    consumer = "quanta-runtime"
    names = sorted(MODULE.REQUIRED_QUANTA_PACKAGES)
    quanta.mkdir()
    (quanta / "Cargo.toml").write_text(
        "[workspace]\nmembers = " + json.dumps([f"crates/{name}" for name in names]) + "\n"
    )
    packages = []
    nodes = []
    for name in names:
        manifest = quanta / "crates" / name / "Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n')
        packages.append(
            {
                "id": name,
                "name": name,
                "manifest_path": str(manifest),
                "source": None,
                "version": "0.1.0",
            }
        )
        nodes.append({"id": name, "dependencies": [], "features": []})
    manifest = workspace / "crates" / consumer / "Cargo.toml"
    manifest.parent.mkdir(parents=True)
    manifest.write_text(f'[package]\nname = "{consumer}"\nversion = "0.1.0"\n')
    (workspace / "Cargo.lock").write_text("nested-lock-authority\n")
    (paired / "Cargo.lock").write_text("unrelated-root-lock\n")
    packages.append(
        {
            "id": consumer,
            "name": consumer,
            "manifest_path": str(manifest),
            "source": None,
            "version": "0.1.0",
        }
    )
    nodes.append({"id": consumer, "dependencies": names, "features": ["index-sdk-ingress"]})
    metadata = {
        "version": 1,
        "workspace_root": str(workspace),
        "packages": packages,
        "resolve": {"nodes": nodes},
    }
    return quanta, paired, consumer, metadata


def validate(resolution):
    quanta, paired, consumer, metadata = resolution
    return MODULE.validate_resolution(
        metadata, quanta_root=quanta, paired_root=paired, consumer=consumer
    )


def test_exact_root_binds_nested_resolver_lock_and_no_local_paths(resolution):
    result = validate(resolution)
    assert result["dependency_lock"]["path"] == "packages/analysis/quanta-v2/Cargo.lock"
    assert result["dependency_lock"]["sha256"] == MODULE._digest(
        resolution[1] / result["dependency_lock"]["path"]
    )
    assert set(item["name"] for item in result["packages"]) == MODULE.REQUIRED_QUANTA_PACKAGES
    assert str(resolution[0].parent) not in json.dumps(result)


def test_wrong_checkout_same_name_same_bytes_is_refused(resolution, tmp_path):
    quanta, _, _, metadata = resolution
    package = metadata["packages"][0]
    original = Path(package["manifest_path"])
    sidecar = tmp_path / "quanta-sidecar" / original.relative_to(quanta)
    sidecar.parent.mkdir(parents=True)
    sidecar.write_bytes(original.read_bytes())
    package["manifest_path"] = str(sidecar)
    with pytest.raises(ValueError, match="another source"):
        validate(resolution)


def test_manifest_symlink_to_other_checkout_is_refused(resolution, tmp_path):
    package = resolution[3]["packages"][0]
    manifest = Path(package["manifest_path"])
    sidecar = tmp_path / "sidecar-Cargo.toml"
    sidecar.write_bytes(manifest.read_bytes())
    manifest.unlink()
    manifest.symlink_to(sidecar)
    with pytest.raises(ValueError, match="escapes"):
        validate(resolution)


def test_canonical_symlink_alias_to_expected_checkout_is_accepted(resolution):
    quanta, paired, consumer, metadata = resolution
    alias = quanta.parent / "quanta-alias"
    alias.symlink_to(quanta, target_is_directory=True)
    metadata["packages"][0]["manifest_path"] = str(
        alias / Path(metadata["packages"][0]["manifest_path"]).relative_to(quanta)
    )
    assert MODULE.validate_resolution(
        metadata, quanta_root=quanta, paired_root=paired, consumer=consumer
    )["packages"]


@pytest.mark.parametrize(
    "source", ["registry+https://example.invalid/index", "git+https://example.invalid/quanta"]
)
def test_non_path_dependency_is_refused_even_with_expected_manifest(resolution, source):
    resolution[3]["packages"][0]["source"] = source
    with pytest.raises(ValueError, match="another source"):
        validate(resolution)


def test_missing_source_is_not_assumed_to_be_local(resolution):
    del resolution[3]["packages"][0]["source"]
    with pytest.raises(ValueError, match="another source"):
        validate(resolution)


def test_kernel_feature_profile_is_admitted_independently(resolution):
    quanta, paired, consumer, metadata = resolution
    kernel = "quanta-runtime-retrieval-kernel"
    manifest = paired / "packages/analysis/quanta-v2/crates" / kernel / "Cargo.toml"
    manifest.parent.mkdir()
    manifest.write_text(f'[package]\nname = "{kernel}"\nversion = "0.1.0"\n')
    metadata["packages"][-1].update(id=kernel, name=kernel, manifest_path=str(manifest))
    metadata["resolve"]["nodes"][-1].update(id=kernel, features=["index-sdk-ingress-surface"])
    result = MODULE.validate_resolution(
        metadata, quanta_root=quanta, paired_root=paired, consumer=kernel
    )
    assert result["consumer"] == kernel
    assert result["consumer_features"] == ["index-sdk-ingress-surface"]


def test_required_package_not_reachable_is_refused(resolution):
    resolution[3]["resolve"]["nodes"][-1]["dependencies"].pop()
    with pytest.raises(ValueError, match="all required"):
        validate(resolution)


@pytest.mark.parametrize(
    "mutation,error",
    [
        ("no_graph", "resolve graph"),
        ("duplicate_package", "duplicate Cargo package"),
        ("duplicate_node", "duplicate Cargo resolve node"),
        ("unknown_edge", "dependency edge"),
        ("missing_node", "lacks a resolve node"),
        ("disabled_feature", "not enabled"),
    ],
)
def test_malformed_or_incomplete_graph_is_refused(resolution, mutation, error):
    metadata = resolution[3]
    if mutation == "no_graph":
        metadata["resolve"] = None
    elif mutation == "duplicate_package":
        metadata["packages"].append(copy.deepcopy(metadata["packages"][0]))
    elif mutation == "duplicate_node":
        metadata["resolve"]["nodes"].append(copy.deepcopy(metadata["resolve"]["nodes"][0]))
    elif mutation == "unknown_edge":
        metadata["resolve"]["nodes"][-1]["dependencies"].append("unknown")
    elif mutation == "missing_node":
        metadata["resolve"]["nodes"].pop(0)
    elif mutation == "disabled_feature":
        metadata["resolve"]["nodes"][-1]["features"] = []
    with pytest.raises(ValueError, match=error):
        validate(resolution)


def test_lock_symlink_outside_workspace_is_refused(resolution):
    lock = resolution[1] / "packages/analysis/quanta-v2/Cargo.lock"
    lock.unlink()
    lock.symlink_to(resolution[1] / "Cargo.lock")
    with pytest.raises(ValueError, match="lock escapes"):
        validate(resolution)


def test_workspace_symlink_escape_is_refused(resolution, tmp_path):
    quanta, paired, _, metadata = resolution
    workspace = paired / "packages/analysis/quanta-v2"
    external = tmp_path / "external-workspace"
    workspace.rename(external)
    workspace.symlink_to(external, target_is_directory=True)
    with pytest.raises(ValueError, match="workspace escapes"):
        validate(resolution)


def test_consumer_manifest_symlink_escape_is_refused(resolution, tmp_path):
    manifest = Path(resolution[3]["packages"][-1]["manifest_path"])
    external = tmp_path / "external-consumer.toml"
    external.write_bytes(manifest.read_bytes())
    manifest.unlink()
    manifest.symlink_to(external)
    with pytest.raises(ValueError, match="consumer escapes"):
        validate(resolution)


def test_duplicate_json_keys_cli_fails_closed(resolution):
    quanta, paired, consumer, _ = resolution
    result = subprocess.run(
        [
            sys.executable,
            str(MODULE_PATH),
            "--quanta-root",
            str(quanta),
            "--paired-root",
            str(paired),
            "--consumer",
            consumer,
        ],
        input='{"version":1,"version":1}',
        text=True,
        capture_output=True,
    )
    assert result.returncode == 1
    assert result.stdout == ""
    assert "duplicate JSON key" in result.stderr


def test_cli_emits_same_validated_mapping(resolution):
    quanta, paired, consumer, metadata = resolution
    result = subprocess.run(
        [
            sys.executable,
            str(MODULE_PATH),
            "--quanta-root",
            str(quanta),
            "--paired-root",
            str(paired),
            "--consumer",
            consumer,
        ],
        input=json.dumps(metadata),
        text=True,
        capture_output=True,
        check=True,
    )
    assert json.loads(result.stdout) == validate(resolution)
