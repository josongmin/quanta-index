"""Pinned candidate bundle installs into the manifest consumed by the CLI."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import subprocess
import sys
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.benchmark.retrieval.symbol_coverage import candidate_grammar_identity
from tools.ci import install_tree_sitter_candidate as installer


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _bundle(tmp_path: Path) -> tuple[Path, Path, str]:
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    descriptor = {
        "schema": installer.SCHEMA,
        "platform": {"Darwin": "macos", "Linux": "linux"}[platform.system()],
        "architecture": {"arm64": "aarch64"}.get(platform.machine(), platform.machine()),
        "grammar_identity": "registry-go@0.25.0;registry-javascript@0.25.0;registry-typescript@0.23.2",
        "go_version": "go1.25.4",
        "node_version": "v25.9.0",
        "typescript_version": "5.9.3",
        "script_sha256": _sha(installer.SCRIPT.read_bytes()),
        "node_libraries": [],
    }
    for key in installer.ARTIFACTS:
        data = key.encode()
        (bundle / key).write_bytes(data)
        descriptor[key] = {"path": key, "sha256": _sha(data)}
    path = tmp_path / "descriptor.json"
    encoded = json.dumps(descriptor, sort_keys=True).encode()
    path.write_bytes(encoded)
    return bundle, path, _sha(encoded)


def test_installer_publishes_cli_manifest_only_after_probe(tmp_path, monkeypatch):
    bundle, descriptor, digest = _bundle(tmp_path)
    observed = []
    monkeypatch.setattr(installer, "_check_dynamic_dependencies", lambda *_args: None)
    monkeypatch.setattr(installer, "_probe", lambda manifest: observed.append(manifest))
    physical_parent = tmp_path / "physical"
    physical_parent.mkdir()
    alias = tmp_path / "platform-tmp"
    alias.symlink_to(physical_parent, target_is_directory=True)
    output, manifest_sha = installer.install(bundle, descriptor, digest, alias / "installed")
    assert output == physical_parent.resolve() / "installed" / "manifest.json"
    manifest = json.loads(output.read_bytes())
    assert observed == [manifest]
    assert _sha(output.read_bytes()) == manifest_sha
    assert all(Path(manifest[key]["path"]).is_absolute() for key in installer.ARTIFACTS)
    assert all(
        Path(manifest[key]["path"]).read_bytes() == key.encode() for key in installer.ARTIFACTS
    )
    assert candidate_grammar_identity(output, manifest_sha).startswith("registry-go@0.25.0;")


@pytest.mark.parametrize("incorrect_batch_span", [False, True])
def test_installer_executes_batch_and_requires_exact_offsets(
    tmp_path, monkeypatch, incorrect_batch_span
):
    bundle, descriptor, digest = _bundle(tmp_path)
    fixture_names = {
        "probe.go": ("One", 15, 18),
        "probe.js": ("local", 23, 28),
        "probe.ts": ("One", 39, 42),
        "probe.tsx": ("One", 39, 42),
    }

    def row(path):
        if path == "malformed.ts":
            return {"status": "parse_failure", "records": [], "names": {}}
        name, start, end = fixture_names[path]
        return {
            "status": "ok",
            "records": [
                {
                    "symbol_id": "sym-v1:" + "a" * 64,
                    "symbol_kind": "function",
                    "repo_relative_path": path,
                    "local_name": name,
                }
            ],
            "names": {"sym-v1:" + "a" * 64: {"name": name, "start_byte": start, "end_byte": end}},
        }

    commands = []

    def probe_output(command, source, _label):
        filename = Path(command[0]).name
        commands.append(filename)
        if filename == "official_binary":
            result = row(command[2])
        else:
            assert filename == "syntax_batch_binary"
            paths = [source["path"] for source in json.loads(source)]
            assert paths == ["probe.js", "probe.ts", "probe.tsx", "malformed.ts"]
            result = [{"path": path, **row(path)} for path in paths]
            if incorrect_batch_span:
                result[0]["names"]["sym-v1:" + "a" * 64]["start_byte"] += 1
        return result

    monkeypatch.setattr(installer, "_check_dynamic_dependencies", lambda *_args: None)
    monkeypatch.setattr(installer, "_probe_output", probe_output)
    destination = tmp_path / "installed"
    if incorrect_batch_span:
        with pytest.raises(ValueError, match="syntax batch differs"):
            installer.install(bundle, descriptor, digest, destination)
        assert not (destination / "manifest.json").exists()
    else:
        assert installer.install(bundle, descriptor, digest, destination)[0].is_file()
    assert commands == ["official_binary"] * 4 + ["syntax_batch_binary"]


def test_probe_timeout_kills_descendant_holding_output_pipe(tmp_path, monkeypatch):
    descendant = tmp_path / "descendant.py"
    descendant.write_text("import time\ntime.sleep(30)\n")
    pid_file = tmp_path / "descendant.pid"
    launcher = tmp_path / "launcher.py"
    launcher.write_text(
        "import subprocess, sys\n"
        "from pathlib import Path\n"
        f"child = subprocess.Popen([sys.executable, {str(descendant)!r}])\n"
        f"Path({str(pid_file)!r}).write_text(str(child.pid))\n"
    )
    monkeypatch.setattr(installer, "PROBE_TIMEOUT_SECONDS", 0.75)
    with pytest.raises(ValueError, match="installed candidate probe failed: held pipe: timeout"):
        installer._probe_output([sys.executable, str(launcher)], "{}", "held pipe")
    assert pid_file.is_file()
    child_pid = int(pid_file.read_text())
    deadline = time.monotonic() + 3
    while True:
        state = subprocess.run(
            ["ps", "-o", "stat=", "-p", str(child_pid)],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.strip()
        if not state or state.startswith("Z"):
            break
        assert time.monotonic() < deadline, f"descendant survived probe timeout: {state}"
        time.sleep(0.05)


def test_installer_rejects_unpinned_and_swapped_artifacts(tmp_path, monkeypatch):
    bundle, descriptor, digest = _bundle(tmp_path)
    monkeypatch.setattr(installer, "_check_dynamic_dependencies", lambda *_args: None)
    monkeypatch.setattr(installer, "_probe", lambda _manifest: None)
    with pytest.raises(ValueError, match="external SHA-256"):
        installer.install(bundle, descriptor, "0" * 64, tmp_path / "bad-pin")
    (bundle / "node").write_bytes(b"swapped")
    with pytest.raises(ValueError, match="artifact digest differs: node"):
        installer.install(bundle, descriptor, digest, tmp_path / "bad-node")
    assert not (tmp_path / "bad-node").exists()


def test_installer_rejects_linked_artifact_and_failed_probe(tmp_path, monkeypatch):
    bundle, descriptor, digest = _bundle(tmp_path)
    (bundle / "node").rename(bundle / "original")
    (bundle / "node").symlink_to(bundle / "original")
    with pytest.raises(ValueError, match="linked or unavailable artifact"):
        installer.install(bundle, descriptor, digest, tmp_path / "linked")
    (bundle / "node").unlink()
    (bundle / "original").rename(bundle / "node")
    monkeypatch.setattr(installer, "_check_dynamic_dependencies", lambda *_args: None)
    monkeypatch.setattr(
        installer, "_probe", lambda _manifest: (_ for _ in ()).throw(ValueError("probe"))
    )
    with pytest.raises(ValueError, match="probe"):
        installer.install(bundle, descriptor, digest, tmp_path / "failed-probe")
    assert not (tmp_path / "failed-probe").exists()


def test_opened_file_rejects_fifo_replaced_at_open(tmp_path, monkeypatch):
    path = tmp_path / "candidate"
    path.write_bytes(b"valid")
    open_file = installer.os.open

    def replace_with_fifo(target, flags, *args, **kwargs):
        if target == path:
            path.unlink()
            os.mkfifo(path)
        return open_file(target, flags, *args, **kwargs)

    monkeypatch.setattr(installer.os, "open", replace_with_fifo)
    with pytest.raises(ValueError, match="bounded regular file"):
        installer._regular_bytes(path, 16)


def test_bundle_intermediate_swap_cannot_escape_opened_directory(tmp_path, monkeypatch):
    bundle = tmp_path / "bundle"
    nested = bundle / "nested"
    nested.mkdir(parents=True)
    (nested / "node").write_bytes(b"exact bytes")
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "node").write_bytes(b"exact bytes")
    open_file = installer.os.open

    def replace_directory(target, flags, *args, **kwargs):
        if target == "nested" and kwargs.get("dir_fd") is not None:
            nested.rename(bundle / "saved")
            nested.symlink_to(outside)
        return open_file(target, flags, *args, **kwargs)

    monkeypatch.setattr(installer.os, "open", replace_directory)
    item = {"path": "nested/node", "sha256": _sha(b"exact bytes")}
    with pytest.raises(ValueError, match="linked or unavailable artifact"):
        installer._artifact(bundle, item, "node")


def test_modified_descriptor_is_rejected_by_external_pin(tmp_path, monkeypatch):
    bundle, descriptor, digest = _bundle(tmp_path)
    document = json.loads(descriptor.read_bytes())
    document["grammar_identity"] += ";modified"
    descriptor.write_text(json.dumps(document))
    monkeypatch.setattr(installer, "_check_dynamic_dependencies", lambda *_args: None)
    monkeypatch.setattr(installer, "_probe", lambda _manifest: None)
    with pytest.raises(ValueError, match="external SHA-256"):
        installer.install(bundle, descriptor, digest, tmp_path / "changed")


def test_macos_dependency_closure_rejects_ambient_dylib(monkeypatch, tmp_path):
    monkeypatch.setattr(installer.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(
        installer.subprocess,
        "run",
        lambda *_args, **_kwargs: SimpleNamespace(
            stdout="node:\n\t@rpath/libnode.dylib (version)\n"
            "\t/opt/homebrew/opt/icu/lib/libicu.dylib (version)\n"
        ),
    )
    with pytest.raises(ValueError, match="unstaged macOS dynamic dependency"):
        installer._check_dynamic_dependencies(tmp_path / "node", {"libnode.dylib"})


def test_linux_dependency_closure_rejects_ambient_library(monkeypatch, tmp_path):
    monkeypatch.setattr(installer.platform, "system", lambda: "Linux")
    monkeypatch.setattr(
        installer.subprocess,
        "run",
        lambda *_args, **_kwargs: SimpleNamespace(
            returncode=0,
            stderr="",
            stdout="libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x0)\n"
            "libnode.so => /opt/node/libnode.so (0x0)\n",
        ),
    )
    with pytest.raises(ValueError, match="unstaged Linux dynamic dependency"):
        installer._check_dynamic_dependencies(tmp_path / "node", set())
