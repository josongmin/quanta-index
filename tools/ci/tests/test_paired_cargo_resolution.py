"""Independent malformed/wrong-checkout counterexamples for the resolver guard."""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

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
    (workspace / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/quanta-runtime"]\n')
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
@pytest.mark.parametrize("temporary", ["default", "custom", "alias"])
def test_cross_repo_script_consumes_pinned_binary_and_rejects_alias_drift(
    resolution, tmp_path, monkeypatch, mutate, temporary
):
    quanta, paired, _, _ = resolution
    repo_root = Path(__file__).resolve().parents[3]
    monkeypatch.delenv("QUANTA_P11_R5_EVIDENCE_ROOT", raising=False)
    if temporary == "default":
        monkeypatch.delenv("TMPDIR", raising=False)
        temporary_root = Path("/tmp").resolve()
    else:
        temporary_root = tmp_path / "temporary root"
        temporary_root.mkdir()
        named_root = temporary_root
        if temporary == "alias":
            named_root = tmp_path / "temporary alias"
            named_root.symlink_to(temporary_root, target_is_directory=True)
        monkeypatch.setenv("TMPDIR", str(named_root))
        temporary_root = temporary_root.resolve()
    for relative in (
        "scripts/verify-repomap-cross-repo.sh",
        "tools/ci/paired_cargo_resolution.py",
        "tools/ci/binary_custody.py",
        "tools/ci/lint/handoff_validation.py",
        "tools/ci/proof_json.py",
    ):
        target = quanta / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((repo_root / relative).read_bytes())
    # This scenario owns binary custody; resolver/selected-run behavior has
    # independent tests and is stubbed so no QBC lane or Rust build is needed.
    (quanta / "tools/ci/paired_cargo_resolution.py").write_text(
        "import argparse, json\n"
        "p=argparse.ArgumentParser()\n"
        "p.add_argument('--consumer'); p.add_argument('--quanta-root'); "
        "p.add_argument('--paired-root'); p.add_argument('--feature'); p.add_argument('--qbc-lane')\n"
        "a=p.parse_args(); print(json.dumps({'consumer':a.consumer,'feature':a.feature}))\n"
    )
    target_dir = tmp_path / "target"
    (target_dir / "release").mkdir(parents=True)
    built = target_dir / "release/quanta-index-searchd"
    built.write_bytes(b"#!/bin/sh\nprintf original\n")
    built.chmod(0o700)
    evidence_parent_record = tmp_path / "evidence-parent.txt"
    (quanta / "tools/ci/paired_r5_result.py").write_text(
        "import argparse, os, pathlib, subprocess\n"
        "p=argparse.ArgumentParser()\n"
        "for k in ('quanta-root','semantica-root','evidence-root','qbc-lane','quanta-head',"
        "'semantica-head','daemon-digest','built-binary','provided-binary','custody-binary',"
        "'runtime-resolution','kernel-resolution'): p.add_argument('--'+k)\n"
        "a=p.parse_args(); pinned=pathlib.Path(a.custody_binary)\n"
        "assert pinned != pathlib.Path(a.built_binary)\n"
        "assert subprocess.check_output([str(pinned)]) == b'original'\n"
        "evidence=pathlib.Path(a.evidence_root)\n"
        f"assert evidence.parent.parent == pathlib.Path({str(temporary_root)!r})\n"
        "assert evidence == evidence.resolve() and evidence.parent.is_dir()\n"
        f"pathlib.Path({str(evidence_parent_record)!r}).write_text(str(evidence.parent))\n"
        f"if {mutate!r}: pathlib.Path({str(built)!r}).write_bytes(b'changed release bytes')\n"
    )
    (quanta / "scripts/cargow").write_text(
        "#!/bin/sh\nprintf '%s\\n' '" + json.dumps({"target_directory": str(target_dir)}) + "'\n"
    )
    (quanta / "scripts/cargow").chmod(0o700)
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    (bin_dir / "just").write_text("#!/bin/sh\nexit 0\n")
    (bin_dir / "just").chmod(0o700)
    for root in (quanta, paired):
        (root / ".gitignore").write_text("__pycache__/\n", encoding="utf-8")
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
    monkeypatch.setenv("QUANTA_P11_R5_QBC_LANE", "registered-resolution")
    completed = subprocess.run(
        ["bash", str(quanta / "scripts/verify-repomap-cross-repo.sh"), str(paired)],
        text=True,
        capture_output=True,
    )
    if evidence_parent_record.exists():
        # The stub does not emit a result. Remove only its recorded fresh parent.
        evidence_parent = Path(evidence_parent_record.read_text())
        assert evidence_parent.parent == temporary_root
        evidence_parent.rmdir()
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
    "failure",
    [
        None,
        "stale",
        "output-change",
        "nonzero",
        "wrong-command",
        "bad-json",
        "unregistered",
        "effective-failure",
        "raw-unobserved",
        "raw-failure",
        "missing-output",
        "owner-change",
        "wrong-path",
        "wrong-nonce",
        "wrong-cwd",
    ],
)
def test_qbc_resolver_uses_own_stable_auxiliary_stdout_and_refuses_bad_owners(
    resolution, tmp_path, monkeypatch, failure
):
    quanta, paired, consumer, metadata = resolution
    state = tmp_path / "state"
    state.mkdir()
    lane = "registered-resolution"
    nonce = ""
    status_calls = 0
    command = [
        "cargo",
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--no-default-features",
        "--manifest-path",
        f"packages/analysis/quanta-v2/crates/{consumer}/Cargo.toml",
        "--features",
        "index-sdk-ingress",
    ]

    def fake_status(source, requested_lane, environment):
        nonlocal status_calls
        status_calls += 1
        assert source == paired and requested_lane == lane
        if failure == "unregistered":
            raise ValueError(f"QBC lane registration missing: {lane}")
        current = status_calls > 1
        run_id = "old" if not current or failure == "stale" else "own"
        if failure == "owner-change" and status_calls == 3:
            run_id = "other"
        output_path = (
            state
            / "execution-roots/owner-key/lanes/registered-resolution/last-completed-run/stdout.log"
        )
        item = {
            "lane": lane,
            "lane_key": lane,
            "execution_root": str(paired),
            "registered": True,
            "receipt": {
                "last_run_id_v1": run_id,
                "run_state": "finished",
                "command": command if failure != "wrong-command" else command[:-1],
                "command_cwd_v1": str(quanta if failure == "wrong-cwd" else paired),
                "last_exit_code": 125 if failure == "effective-failure" else 0,
                "command_exit_code_v1": (
                    None if failure == "raw-unobserved" else 1 if failure == "raw-failure" else 0
                ),
                "last_stdout_path_v1": str(quanta if failure == "wrong-path" else output_path),
            },
            "meta": {
                "user_meta": {
                    "paired_r5_resolution_nonce": "other" if failure == "wrong-nonce" else nonce
                },
                "command_context": {"last_run_id_v1": run_id},
            },
        }
        return ({"state_root": str(state), "execution_root_key": "owner-key"}, item)

    def fake_run(argv, **_kwargs):
        nonlocal nonce
        assert argv[:4] == [str(paired / "scripts/quanta-build-cli"), "cargo", "--lane", lane]
        nonce = argv[5].split("=", 1)[1]
        assert argv[6:] == ["--", *command[1:]]
        return SimpleNamespace(returncode=1 if failure == "nonzero" else 0, stdout=b"", stderr=b"")

    raw = b"{" if failure == "bad-json" else json.dumps(metadata).encode()
    reads = 0

    def fake_read(_path):
        nonlocal reads
        reads += 1
        if failure == "missing-output":
            raise FileNotFoundError("metadata output absent")
        if failure == "output-change" and reads == 2:
            return b"changed", (1, 2, 3, 4, 5)
        return raw, (1, 2, len(raw), 4, 5)

    monkeypatch.setattr(MODULE, "_frozen_head", lambda _root: "f" * 40)
    monkeypatch.setattr(MODULE, "_status", fake_status)
    monkeypatch.setattr(MODULE, "_read_bounded_regular", fake_read)
    monkeypatch.setattr(MODULE.subprocess, "run", fake_run)

    def invoke():
        return MODULE.resolve_from_qbc(
            quanta_root=quanta,
            paired_root=paired,
            consumer=consumer,
            feature="index-sdk-ingress",
            lane=lane,
        )

    if failure in (None, "raw-unobserved"):
        assert invoke()["consumer"] == consumer
        assert reads == 2 and status_calls == 3
    else:
        with pytest.raises(ValueError):
            invoke()
        if failure == "unregistered":
            assert status_calls == 1
        if failure == "bad-json":
            assert reads == 1


def test_qbc_auxiliary_output_reader_requires_bounded_regular_single_link(tmp_path, monkeypatch):
    output = tmp_path / "stdout.log"
    output.write_bytes(b'{"version":1}')
    assert MODULE._read_bounded_regular(output)[0] == output.read_bytes()
    alias = tmp_path / "alias.log"
    alias.symlink_to(output)
    with pytest.raises(ValueError, match="alias"):
        MODULE._read_bounded_regular(alias)
    hardlink = tmp_path / "hardlink.log"
    os.link(output, hardlink)
    with pytest.raises(ValueError, match="regular"):
        MODULE._read_bounded_regular(output)
    hardlink.unlink()
    monkeypatch.setattr(MODULE, "QBC_METADATA_MAX_BYTES", 4)
    with pytest.raises(ValueError, match="bounded"):
        MODULE._read_bounded_regular(output)


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
