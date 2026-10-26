"""Managed build refuses failed, stale, escaped and drifting source artifacts."""

from __future__ import annotations

import hashlib
import io
import json
import stat
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import causal_cost_build as build
from tools.benchmark.retrieval.tool_custody import TOOL_NAMES


@pytest.fixture
def build_fixture(tmp_path, monkeypatch):
    repo = tmp_path / "source"
    (repo / "scripts").mkdir(parents=True)
    wrapper = repo / "scripts/cargow"
    wrapper.write_bytes(b"#!/bin/bash\nexit 0\n")
    wrapper.chmod(0o700)
    paths = {}
    for name in TOOL_NAMES:
        path = tmp_path / name
        path.write_bytes(b"#!/bin/bash\nexit 0\n")
        path.chmod(0o700)
        paths[name] = path
    out = tmp_path / "build"
    state = {
        "head": "a" * 40,
        "source": {"identity_sha256": "b" * 64},
        "calls": [],
        "fault": None,
        "backend": "legacy",
        "build_env": None,
    }
    monkeypatch.setattr(build, "resolve_tool_paths", lambda _cwd, _env: paths.copy())
    monkeypatch.setattr(build, "source_capture", lambda *_args, **_kw: state["source"].copy())

    def run(argv, **kwargs):
        state["calls"].append(argv)
        if "rev-parse" in argv:
            return subprocess.CompletedProcess(argv, 0, (state["head"] + "\n").encode(), b"")
        if "metadata" in argv:
            return subprocess.CompletedProcess(argv, 0, b'{"packages":[]}', b"")
        native = state["backend"] == "index-native"
        assert argv[1:] == list(build.NATIVE_BUILD_ARGS if native else build.BUILD_ARGS)
        assert kwargs["env"]["CARGO_TARGET_DIR"] == str(out / "target")
        assert kwargs["env"]["CARGO_BUILD_JOBS"] == "1"
        assert kwargs["env"]["RUSTC_WRAPPER"] == ""
        state["build_env"] = kwargs["env"].copy()
        target = out / "target/release"
        target.mkdir(parents=True)
        binary = target / "scale_matrix"
        binary.write_bytes(b"#!/bin/bash\nexit 0\n")
        binary.chmod(0o700)
        row = {
            "reason": "compiler-artifact",
            "target": {"name": "scale_matrix", "kind": ["bin"]},
            "manifest_path": str(repo / "crates/quanta-index-searchd-harness/Cargo.toml"),
            "executable": str(binary),
            "fresh": False,
            "profile": {"test": False},
        }
        fault = state["fault"]
        if fault == "fresh":
            row["fresh"] = True
        elif fault == "escape":
            row["executable"] = str(paths["cargo"])
        elif fault == "manifest":
            row["manifest_path"] = str(repo / "other/Cargo.toml")
        elif fault == "source":
            state["source"]["identity_sha256"] = "c" * 64
        rows = [row, *([row] if fault == "duplicate" else [])]
        if native:
            daemon = target / "quanta-index-searchd"
            daemon.write_bytes(b"#!/bin/bash\nexit 2\n")
            daemon.chmod(0o700)
            daemon_row = {
                "reason": "compiler-artifact",
                "target": {"name": "quanta-index-searchd", "kind": ["bin"]},
                "manifest_path": str(repo / "crates/quanta-index-searchd-runtime/Cargo.toml"),
                "executable": str(daemon),
                "fresh": False,
                "profile": {"test": False},
            }
            if fault == "native_manifest":
                daemon_row["manifest_path"] = str(repo / "other/Cargo.toml")
            elif fault == "native_fresh":
                daemon_row["fresh"] = True
            elif fault == "native_outside":
                daemon_row["executable"] = str(paths["cargo"])
            elif fault == "native_wrong_profile_dir":
                wrong_dir = out / "target/bench"
                wrong_dir.mkdir()
                wrong_binary = wrong_dir / "quanta-index-searchd"
                wrong_binary.write_bytes(daemon.read_bytes())
                wrong_binary.chmod(0o700)
                daemon_row["executable"] = str(wrong_binary)
            if fault != "native_missing":
                rows.append(daemon_row)
            if fault == "native_duplicate":
                rows.append(daemon_row.copy())
        rows.append(
            {"reason": "build-finished", "success": fault not in ("terminal", "native_terminal")}
        )
        kwargs["stdout"].write(b"".join(json.dumps(x).encode() + b"\n" for x in rows))
        return subprocess.CompletedProcess(argv, 1 if fault == "exit" else 0)

    monkeypatch.setattr(build.subprocess, "run", run)
    return repo, out, state


def test_fresh_managed_build_binds_actual_binary_and_fixed_recipe(build_fixture):
    repo, out, state = build_fixture
    result = build.build_scale(repo, out)
    assert result["status"] == "OBSERVED_LOCAL_BUILD"
    assert result["source_revision"] == "a" * 40
    assert result["command"] == [str(repo / "scripts/cargow"), *build.BUILD_ARGS]
    assert Path(result["binary"]).read_bytes() == b"#!/bin/bash\nexit 0\n"
    assert json.loads((out / "build-receipt.json").read_text()) == result
    assert len(state["calls"]) == 3


def test_index_native_build_binds_two_fresh_bench_binaries_from_one_cargo_run(build_fixture):
    repo, out, state = build_fixture
    state["backend"] = "index-native"
    result = build.build_scale(repo, out, backend="index-native")
    assert result["command"] == [str(repo / "scripts/cargow"), *build.NATIVE_BUILD_ARGS]
    assert result["backend"] == "index-native"
    assert result["build_profile"] == "bench"
    assert result["binary"] == str(out / "scale_matrix")
    assert result["native_daemon"]["binary"] == str(out / "quanta-index-searchd")
    assert Path(result["binary"]).read_bytes() == b"#!/bin/bash\nexit 0\n"
    assert Path(result["native_daemon"]["binary"]).read_bytes() == b"#!/bin/bash\nexit 2\n"
    assert stat.S_IMODE(Path(result["binary"]).stat().st_mode) == 0o500
    assert stat.S_IMODE(Path(result["native_daemon"]["binary"]).stat().st_mode) == 0o500
    assert result["binary_epoch"]["sha256"] == result["binary_sha256"]
    assert (
        result["native_daemon"]["binary_epoch"]["sha256"]
        == result["native_daemon"]["binary_sha256"]
    )
    assert json.loads((out / "build-receipt.json").read_text()) == result
    assert len(state["calls"]) == 3


@pytest.mark.parametrize("resource_wait", ["601", "+601", " 601 "])
def test_resource_wait_override_reaches_managed_build_and_receipt(
    build_fixture, monkeypatch, resource_wait
):
    repo, out, state = build_fixture
    monkeypatch.setenv("QUANTA_INDEX_RESOURCE_WAIT_SECONDS", resource_wait)
    monkeypatch.setenv("QUANTA_INDEX_RESOURCE_ADMISSION", "0")
    monkeypatch.setenv("CARGO_TARGET_DIR", "/tmp/foreign-target")
    receipt = build.build_scale(repo, out)
    assert state["build_env"]["QUANTA_INDEX_RESOURCE_WAIT_SECONDS"] == resource_wait
    assert state["build_env"]["QUANTA_INDEX_RESOURCE_ADMISSION"] == "auto"
    assert state["build_env"]["CARGO_TARGET_DIR"] == str(out / "target")
    assert state["build_env"]["CARGO_BUILD_JOBS"] == "1"
    assert receipt["environment_sha256"]["QUANTA_INDEX_RESOURCE_WAIT_SECONDS"] == (
        hashlib.sha256(resource_wait.encode()).hexdigest()
    )


@pytest.mark.parametrize("value", ["", "0", "000", "-1", "1.5", "not-an-integer"])
def test_invalid_resource_wait_is_refused_before_output(build_fixture, monkeypatch, value):
    repo, out, state = build_fixture
    monkeypatch.setenv("QUANTA_INDEX_RESOURCE_WAIT_SECONDS", value)
    with pytest.raises(ValueError, match="resource wait must be a positive integer"):
        build.build_scale(repo, out)
    assert not out.exists()
    assert not state["calls"]


@pytest.mark.parametrize(
    "fault", ["fresh", "escape", "manifest", "source", "duplicate", "terminal", "exit"]
)
def test_refusal_never_writes_a_success_receipt(build_fixture, fault):
    repo, out, state = build_fixture
    state["fault"] = fault
    with pytest.raises(ValueError):
        build.build_scale(repo, out)
    assert not (out / "build-receipt.json").exists()
    assert (out / "build.stdout").exists()


@pytest.mark.parametrize(
    "fault",
    [
        "native_missing",
        "native_manifest",
        "native_duplicate",
        "native_fresh",
        "native_outside",
        "native_wrong_profile_dir",
        "native_terminal",
    ],
)
def test_index_native_refuses_incomplete_or_unbound_pair(build_fixture, fault):
    repo, out, state = build_fixture
    state.update(backend="index-native", fault=fault)
    with pytest.raises(ValueError):
        build.build_scale(repo, out, backend="index-native")
    assert not (out / "build-receipt.json").exists()
    assert (out / "build.stdout").exists()


def test_unknown_backend_is_rejected_before_io(build_fixture):
    repo, out, state = build_fixture
    with pytest.raises(ValueError, match="unknown managed build backend"):
        build.build_scale(repo, out, backend="unknown")
    assert not out.exists()
    assert not state["calls"]


def test_external_path_dependency_is_not_covered_by_git_tree(tmp_path):
    external = tmp_path / "dependency"
    external.mkdir()
    with pytest.raises(ValueError, match="external path dependency"):
        build._dependency_inputs(
            {"packages": [{"manifest_path": str(external / "Cargo.toml"), "source": None}]},
            tmp_path / "repo",
        )


def test_dependency_body_changes_alter_binding(tmp_path):
    external = tmp_path / "registry/package"
    external.mkdir(parents=True)
    body = external / "lib.rs"
    body.write_text("old")
    metadata = {
        "packages": [
            {
                "manifest_path": str(external / "Cargo.toml"),
                "source": "registry+https://example.invalid",
            }
        ]
    }
    before = build._dependency_inputs(metadata, tmp_path / "repo")
    body.write_text("new")
    assert build._dependency_inputs(metadata, tmp_path / "repo") != before


def test_shell_hook_refusal_precedes_output(build_fixture, monkeypatch):
    repo, out, _state = build_fixture
    monkeypatch.setenv("BASH_ENV", "/tmp/injected")
    with pytest.raises(ValueError, match="startup"):
        build.build_scale(repo, out)
    assert not out.exists()


def test_cargo_stream_can_exceed_one_control_document(build_fixture):
    repo, out, _state = build_fixture
    build.build_scale(repo, out)
    original = (out / "build.stdout").read_bytes()
    # A large build has many individually small diagnostic rows. Keep the
    # same independently valid artifact and terminal result at the end.
    row = json.dumps({"reason": "compiler-message", "message": "x" * 1024}).encode() + b"\n"
    stream = io.BytesIO(row * (build.CONTROL_DOCUMENT_BYTES // len(row) + 1) + original)
    assert build._artifact(stream, repo, out) == out / "target/release/scale_matrix"


def test_cargo_stream_refuses_one_oversized_row(build_fixture):
    repo, out, _state = build_fixture
    with pytest.raises(ValueError, match="row exceeds"):
        build._artifact(io.BytesIO(b"{" + b"x" * build.CONTROL_DOCUMENT_BYTES), repo, out)


def test_cargo_stream_refuses_trailing_output_after_terminal(build_fixture):
    repo, out, _state = build_fixture
    build.build_scale(repo, out)
    original = (out / "build.stdout").read_bytes()
    with pytest.raises(ValueError, match="after Cargo terminal"):
        build._artifact(io.BytesIO(original + b"late wrapper output\n"), repo, out)


@pytest.mark.parametrize("row", [b"not JSON\n", b"\n"])
@pytest.mark.parametrize("position", ["before", "inside"])
def test_cargo_stream_refuses_non_json_rows_before_terminal(build_fixture, row, position):
    repo, out, _state = build_fixture
    build.build_scale(repo, out)
    original = (out / "build.stdout").read_bytes()
    if position == "before":
        corrupted = row + original
    else:
        first, terminal = original.split(b"\n", 1)
        corrupted = first + b"\n" + row + terminal
    with pytest.raises(ValueError, match="non-JSON row"):
        build._artifact(io.BytesIO(corrupted), repo, out)
