"""Independent malformed/wrong-checkout counterexamples for the resolver guard."""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from tools.ci import binary_custody

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


@pytest.mark.parametrize("alias", [False, True])
def test_binary_custody_detects_shared_output_replacement(tmp_path, alias):
    built = tmp_path / "built"
    built.write_bytes(b"#!/bin/sh\nprintf '%s' original\n")
    built.chmod(0o700)
    provided = built if alias else tmp_path / "provided"
    if not alias:
        provided.write_bytes(built.read_bytes())
        provided.chmod(0o700)
    directory = tmp_path / "custody"
    directory.mkdir(mode=0o700)
    pinned = directory / "daemon"
    digest = binary_custody.pin(built, provided, pinned)
    binary_custody.verify(digest, [built, provided, pinned])
    built.write_bytes(b"#!/bin/sh\nprintf '%s' replaced\n")
    # The process still uses the admitted copy, not the shared target output.
    assert subprocess.check_output([str(pinned)]) == b"original"
    with pytest.raises(ValueError, match="changed"):
        binary_custody.verify(digest, [built, provided, pinned])


def test_binary_custody_refuses_symlink_and_nonprivate_destination(tmp_path):
    built = tmp_path / "built"
    built.write_bytes(b"#!/bin/sh\nexit 0\n")
    built.chmod(0o700)
    alias = tmp_path / "alias"
    alias.symlink_to(built)
    directory = tmp_path / "custody"
    directory.mkdir(mode=0o700)
    pinned = directory / "daemon"
    with pytest.raises(OSError):
        binary_custody.pin(built, alias, pinned)
    assert not pinned.exists()
    directory.chmod(0o755)
    with pytest.raises(ValueError, match="private"):
        binary_custody.pin(built, built, pinned)
    assert not pinned.exists()


@pytest.mark.parametrize("mutate", [False, True])
def test_cross_repo_script_consumes_pinned_binary_and_rejects_alias_drift(
    resolution, tmp_path, monkeypatch, mutate
):
    quanta, paired, _, runtime_metadata = resolution
    repo_root = Path(__file__).resolve().parents[3]
    kernel = "quanta-runtime-retrieval-kernel"
    kernel_manifest = paired / "packages/analysis/quanta-v2/crates" / kernel / "Cargo.toml"
    kernel_manifest.parent.mkdir()
    kernel_manifest.write_text(f'[package]\nname = "{kernel}"\nversion = "0.1.0"\n')
    kernel_metadata = copy.deepcopy(runtime_metadata)
    kernel_metadata["packages"][-1].update(
        id=kernel, name=kernel, manifest_path=str(kernel_manifest)
    )
    kernel_metadata["resolve"]["nodes"][-1].update(
        id=kernel, features=["index-sdk-ingress-surface"]
    )
    for relative in (
        "scripts/verify-repomap-cross-repo.sh",
        "tools/ci/paired_cargo_resolution.py",
        "tools/ci/binary_custody.py",
        "tools/ci/lint/handoff_validation.py",
    ):
        target = quanta / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((repo_root / relative).read_bytes())
    target_dir = tmp_path / "target"
    (target_dir / "release").mkdir(parents=True)
    built = target_dir / "release/quanta-index-searchd"
    built.write_bytes(b"#!/bin/sh\nprintf original\n")
    built.chmod(0o700)
    (quanta / "scripts/cargow").write_text(
        "#!/bin/sh\nprintf '%s\\n' '" + json.dumps({"target_directory": str(target_dir)}) + "'\n"
    )
    (quanta / "scripts/cargow").chmod(0o700)
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    (bin_dir / "just").write_text("#!/bin/sh\nexit 0\n")
    (bin_dir / "just").chmod(0o700)
    runtime_target = "index_sdk_ingress_live_repomap_roundtrip_survives_runtime_restart_v1"
    kernel_target = "index_sdk_ingress::terminal_receipt_v1::tests::repomap_v2_receipts_require_exact_full_bundle_and_transition_v2"
    driver = tmp_path / "driver.py"
    driver.write_text(
        "import json, os, pathlib, subprocess, sys\n"
        f"runtime = {runtime_metadata!r}\nkernel = {kernel_metadata!r}\n"
        "args = sys.argv[1:]\n"
        "if 'metadata' in args:\n"
        "    print(json.dumps(kernel if any('quanta-runtime-retrieval-kernel' in arg for arg in args) else runtime))\n"
        "elif '--list' in args:\n"
        f"    print(({kernel_target!r} if 'quanta-runtime-retrieval-kernel' in args else {runtime_target!r}) + ': test')\n"
        f"elif {runtime_target!r} in args:\n"
        "    pinned = pathlib.Path(os.environ['QUANTA_INDEX_SEARCHD_BIN'])\n"
        f"    assert pinned != pathlib.Path({str(built)!r})\n"
        "    assert subprocess.check_output([str(pinned)]) == b'original'\n"
        f"    if {mutate!r}: pathlib.Path({str(built)!r}).write_bytes(b'changed release bytes')\n"
    )
    launcher = paired / "scripts/quanta-build-cli"
    launcher.parent.mkdir()
    launcher.write_text(f'#!/bin/sh\nexec python3 "{driver}" "$@"\n')
    launcher.chmod(0o700)
    for root in (quanta, paired):
        for args in (
            ["init", "-q"],
            ["add", "."],
            [
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        ):
            subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)
    monkeypatch.setenv("PATH", str(bin_dir) + os.pathsep + os.environ["PATH"])
    monkeypatch.setenv("QUANTA_INDEX_SEARCHD_BIN", str(built))
    completed = subprocess.run(
        ["bash", str(quanta / "scripts/verify-repomap-cross-repo.sh"), str(paired)],
        text=True,
        capture_output=True,
    )
    if mutate:
        assert completed.returncode != 0
        assert "release daemon bytes changed" in completed.stderr
    else:
        assert completed.returncode == 0, completed.stderr
        assert "paired-daemon-sha256:" in completed.stdout


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
