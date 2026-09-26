"""Public command-selection and file-epoch oracles for local tool custody."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tools.benchmark.retrieval import tool_custody


def executable(path: Path, body: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("#!/bin/sh\n" + body + "\n")
    path.chmod(0o700)
    return path


def selected_tools(tmp_path: Path) -> dict:
    rows = {}
    for name in (*tool_custody.TOOL_NAMES, "cargow"):
        if name == "python":
            path = Path(sys.executable).absolute()
        elif name == "bash":
            path = Path(shutil.which("bash")).resolve()
        else:
            path = executable(tmp_path / "selected" / name, f"printf 'selected-{name}\\n'")
        real = path.resolve(strict=True)
        rows[name] = {"path": str(path), "realpath": str(real),
                      "sha256": hashlib.sha256(real.read_bytes()).hexdigest(), "version": "fixture"}
    return rows


def custody(tmp_path: Path, *, environment=None):
    rows = selected_tools(tmp_path)
    return tool_custody.ToolCustody.create(tmp_path, tmp_path / "private-bin", tools=rows,
                                           environment=environment or dict(os.environ))


def test_private_path_executes_selected_cargo_not_wrong_ambient_cargo(tmp_path):
    wrong = executable(tmp_path / "wrong" / "cargo", "printf 'wrong-cargo\\n'")
    original = dict(os.environ, PATH=str(wrong.parent), RUSTC_WRAPPER="wrong-wrapper",
                    RUSTC_WORKSPACE_WRAPPER="wrong-wrapper", QUANTA_INDEX_SCCACHE="1")
    guard = custody(tmp_path, environment=original)
    guard.check()
    output = subprocess.check_output(["cargo"], env=guard.environment(), text=True)
    guard.check()
    assert output == "selected-cargo\n"
    assert (guard.bin_dir.stat().st_mode & 0o777) == 0o700
    assert guard.environment()["RUSTC"] == guard.tools()["rustc"]["realpath"]
    assert guard.environment()["RUSTC_WRAPPER"] == ""
    assert guard.environment()["RUSTC_WORKSPACE_WRAPPER"] == ""
    assert guard.environment()["QUANTA_INDEX_SCCACHE"] == "0"


def test_rustup_selects_actual_tools_in_root_with_inherited_toolchain(tmp_path):
    ambient = tmp_path / "ambient"
    cargo = executable(tmp_path / "real" / "cargo", "printf 'real-cargo\\n'")
    rustc = executable(tmp_path / "real" / "rustc", "printf 'real-rustc\\n'")
    log = tmp_path / "selection-log"
    executable(ambient / "cargo", "exit 99")
    executable(ambient / "rustc", "exit 99")
    for name in ("cargo-nextest", "git", "bash", "just"):
        executable(ambient / name, "exit 0")
    executable(ambient / "rustup", f'''printf '%s|%s|%s\\n' "$PWD" "$RUSTUP_TOOLCHAIN" "$2" >> '{log}'
case "$2" in cargo) printf '%s\\n' '{cargo}' ;; rustc) printf '%s\\n' '{rustc}' ;; *) exit 3 ;; esac''')
    paths = tool_custody.resolve_tool_paths(tmp_path, {"PATH": str(ambient),
                                                     "RUSTUP_TOOLCHAIN": "fixed-toolchain"})
    assert paths["cargo"] == cargo.resolve()
    assert paths["rustc"] == rustc.resolve()
    assert log.read_text().splitlines() == [
        f"{tmp_path}|fixed-toolchain|cargo", f"{tmp_path}|fixed-toolchain|rustc"]


def test_private_python_shim_preserves_original_venv_prefix(tmp_path):
    guard = custody(tmp_path)
    expected = subprocess.check_output([sys.executable, "-c", "import sys; print(sys.prefix)"], text=True)
    guard.check()
    observed = subprocess.check_output(["python3", "-c", "import sys; print(sys.prefix)"],
                                       env=guard.environment(), text=True)
    guard.check()
    assert observed == expected
    assert guard.tools()["python"]["path"] == str(Path(sys.executable).absolute())
    assert not (guard.bin_dir / "python3").is_symlink()


@pytest.mark.parametrize("mutation", ["retarget", "retarget_restore", "content_restore", "shim"])
def test_epoch_rejects_tool_or_alias_change_even_when_bytes_restored(tmp_path, mutation):
    guard = custody(tmp_path)
    alias = guard.bin_dir / "cargo"
    original = Path(guard.tools()["cargo"]["realpath"])
    if mutation in ("retarget", "retarget_restore"):
        replacement = executable(tmp_path / "replacement", "exit 0")
        alias.unlink()
        alias.symlink_to(replacement)
        if mutation == "retarget_restore":
            alias.unlink()
            alias.symlink_to(original)
    elif mutation == "content_restore":
        data, info = original.read_bytes(), original.stat()
        original.write_bytes(data + b"# changed\n")
        original.write_bytes(data)
        os.utime(original, ns=(info.st_atime_ns, info.st_mtime_ns))
    else:
        (guard.bin_dir / "python3").write_text("#!/bin/sh\nexit 0\n")
    with pytest.raises(tool_custody.ToolCustodyError, match="epoch changed"):
        guard.check()


def test_parent_directory_symlink_retarget_and_restore_is_rejected(tmp_path):
    rows = selected_tools(tmp_path)
    real = Path(rows["cargo"]["realpath"])
    parent_alias = tmp_path / "parent-alias"
    parent_alias.symlink_to(real.parent, target_is_directory=True)
    rows["cargo"]["path"] = str(parent_alias / "cargo")
    guard = tool_custody.ToolCustody.create(tmp_path, tmp_path / "private-bin", tools=rows,
                                           environment=dict(os.environ))
    other = tmp_path / "other"
    other.mkdir()
    parent_alias.unlink()
    parent_alias.symlink_to(other, target_is_directory=True)
    parent_alias.unlink()
    parent_alias.symlink_to(real.parent, target_is_directory=True)
    with pytest.raises(tool_custody.ToolCustodyError, match="epoch changed"):
        guard.check()


def test_offline_record_validation_does_not_require_producer_paths(tmp_path):
    guard = custody(tmp_path)
    record = json.loads(json.dumps(guard.record()))
    for row in record["tools"].values():
        # Archive paths remain references; their files need not exist at replay.
        assert row["path"].startswith("/")
    shutil.rmtree(guard.bin_dir)
    tool_custody.validate_record(record)
    with pytest.raises(tool_custody.ToolCustodyError):
        guard.check()


def test_metadata_with_wrong_selected_bytes_is_refused_before_alias_creation(tmp_path):
    rows = selected_tools(tmp_path)
    rows["cargo"]["sha256"] = "0" * 64
    with pytest.raises(tool_custody.ToolCustodyError, match="differs from local file"):
        tool_custody.ToolCustody.create(tmp_path, tmp_path / "private-bin", tools=rows)
    assert not (tmp_path / "private-bin").exists()


@pytest.mark.parametrize("mutation", ["missing_shim", "empty_chain", "bool_stat", "wrong_binding", "malformed_digest"])
def test_offline_validation_refuses_partial_or_malformed_epoch(tmp_path, mutation):
    record = custody(tmp_path).record()
    row = record["tools"]["cargo"]
    epoch = record["epochs"][row["path"]]
    if mutation == "missing_shim":
        record["epochs"].pop(str(Path(record["private_bin"]) / "python3"))
    elif mutation == "empty_chain":
        epoch["invocation_chain"] = []
    elif mutation == "bool_stat":
        epoch["stat"]["ctime_ns"] = True
    elif mutation == "wrong_binding":
        epoch["sha256"] = "0" * 64
    else:
        record["epochs"][str(Path(record["private_bin"]) / "python3")]["sha256"] = "garbage"
    with pytest.raises(tool_custody.ToolCustodyError):
        tool_custody.validate_record(record)


def test_selection_refuses_rustup_nonzero_or_missing_tool(tmp_path):
    ambient = tmp_path / "ambient"
    executable(ambient / "rustup", "exit 7")
    with pytest.raises(tool_custody.ToolCustodyError, match="could not select cargo"):
        tool_custody.resolve_tool_paths(tmp_path, {"PATH": str(ambient)})
    with pytest.raises(tool_custody.ToolCustodyError, match="unavailable: rustup"):
        tool_custody.resolve_tool_paths(tmp_path, {"PATH": str(tmp_path / "empty")})


def test_inflight_growth_is_rejected_after_bounded_initial_size_read(tmp_path, monkeypatch):
    path = executable(tmp_path / "growing-tool", "exit 0")
    real_sha256 = hashlib.sha256
    reads = []

    class GrowingDigest:
        def __init__(self):
            self.digest = real_sha256()

        def update(self, data):
            reads.append(len(data))
            self.digest.update(data)
            with path.open("ab") as stream:
                stream.write(b"# ongoing append\n")

        def hexdigest(self):
            return self.digest.hexdigest()

    initial_size = path.stat().st_size
    monkeypatch.setattr(tool_custody.hashlib, "sha256", GrowingDigest)
    with pytest.raises(tool_custody.ToolCustodyError, match="changed while reading"):
        tool_custody._epoch(path)
    assert sum(reads) == initial_size


@pytest.mark.parametrize("name", ["cargo", "python3", "git", "command", "source"])
def test_exported_function_can_override_real_pinned_bash_but_admission_refuses(tmp_path, name):
    guard = custody(tmp_path)
    environment = guard.environment()
    environment[f"BASH_FUNC_{name}%%"] = "() { printf 'ambient-exported-function'; }"
    bash = guard.tools()["bash"]["path"]
    guard.check()
    # This is a real shell oracle: file identity alone does not guarantee
    # that Bash resolves a command or builtin to its recorded implementation.
    output = subprocess.check_output([bash, "-c", f"{name} ignored"], env=environment, text=True)
    guard.check()
    assert output == "ambient-exported-function"
    with pytest.raises(tool_custody.ToolCustodyError, match="Bash function exports"):
        tool_custody.validate_environment(environment)
    with pytest.raises(tool_custody.ToolCustodyError, match="Bash function exports"):
        tool_custody.ToolCustody.create(tmp_path, tmp_path / "refused-bin", tools=selected_tools(tmp_path),
                                       environment=environment)
    assert not (tmp_path / "refused-bin").exists()


@pytest.mark.parametrize("key", ["BASH_FUNC_cargo%%", "BASH_FUNC_", "BASH_FUNC_%%",
                                 "BASH_FUNC_cargo", "BASH_FUNC_bad-name%%"])
@pytest.mark.parametrize("value", ["", "not-a-function"])
def test_entire_export_namespace_is_refused_even_empty_or_malformed(tmp_path, monkeypatch, key, value):
    environment = {"PATH": str(tmp_path), key: value}
    invoked = []

    def forbidden(*args, **kwargs):
        invoked.append(args)
        raise AssertionError("selection must not invoke an executable")

    monkeypatch.setattr(tool_custody.subprocess, "run", forbidden)
    with pytest.raises(tool_custody.ToolCustodyError, match="Bash function exports"):
        tool_custody.resolve_tool_paths(tmp_path, environment)
    assert not invoked


@pytest.mark.parametrize("name", ["ENV", "BASH_ENV"])
def test_shell_startup_settings_are_refused_before_selection_and_creation(tmp_path, monkeypatch, name):
    marker = tmp_path / "startup-ran"
    startup = tmp_path / "startup"
    startup.write_text(f"printf hijacked > '{marker}'\n")
    environment = dict(os.environ, **{name: str(startup)})
    invoked = []

    def forbidden(*args, **kwargs):
        invoked.append(args)
        raise AssertionError("selection must not invoke an executable")

    monkeypatch.setattr(tool_custody.subprocess, "run", forbidden)
    with pytest.raises(tool_custody.ToolCustodyError, match="shell startup setting"):
        tool_custody.resolve_tool_paths(tmp_path, environment)
    with pytest.raises(tool_custody.ToolCustodyError, match="shell startup setting"):
        tool_custody.ToolCustody.create(tmp_path, tmp_path / "refused-bin", tools=selected_tools(tmp_path),
                                       environment=environment)
    assert not invoked and not marker.exists() and not (tmp_path / "refused-bin").exists()


def test_empty_shell_startup_settings_preserve_normal_execution(tmp_path):
    environment = dict(os.environ, ENV="", BASH_ENV="")
    tool_custody.validate_environment(environment)
    guard = custody(tmp_path, environment=environment)
    output = subprocess.check_output([guard.tools()["bash"]["path"], "-c", "cargo"],
                                     env=guard.environment(), text=True)
    guard.check()
    assert output == "selected-cargo\n"


def test_additional_executable_binding_and_offline_record(tmp_path):
    guard = custody(tmp_path)
    path = executable(tmp_path / "native-reference", "printf 'native-reference\\n'")
    expected = hashlib.sha256(path.read_bytes()).hexdigest()
    first = guard.bind_executable(path, expected_sha256=expected)
    assert first["sha256"] == expected
    assert first == guard.bind_executable(path)
    first["sha256"] = "0" * 64
    assert guard.bind_executable(path)["sha256"] == expected
    output = subprocess.check_output([path], env=guard.environment(), text=True)
    guard.check()
    assert output == "native-reference\n"
    record = guard.record()
    path.unlink()
    tool_custody.validate_record(record)
    with pytest.raises(tool_custody.ToolCustodyError, match="unavailable"):
        guard.check()


def test_additional_executable_wrong_digest_is_not_registered(tmp_path):
    guard = custody(tmp_path)
    path = executable(tmp_path / "native-reference", "exit 0")
    with pytest.raises(tool_custody.ToolCustodyError, match="digest differs"):
        guard.bind_executable(path, expected_sha256="0" * 64)
    assert str(path) not in guard.record()["epochs"]


def test_additional_executable_changed_then_restored_cannot_rebind(tmp_path):
    guard = custody(tmp_path)
    path = executable(tmp_path / "native-reference", "exit 0")
    first = guard.bind_executable(path)
    original, info = path.read_bytes(), path.stat()
    path.write_bytes(original + b"# changed\n")
    path.write_bytes(original)
    os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
    with pytest.raises(tool_custody.ToolCustodyError, match="epoch changed"):
        guard.bind_executable(path, expected_sha256=first["sha256"])
    with pytest.raises(tool_custody.ToolCustodyError, match="epoch changed"):
        guard.check()


@pytest.mark.parametrize("path", [Path(""), Path("relative-executable")])
def test_additional_executable_requires_absolute_path(tmp_path, path):
    guard = custody(tmp_path)
    with pytest.raises(tool_custody.ToolCustodyError, match="must be absolute"):
        guard.bind_executable(path)


@pytest.mark.parametrize("digest", ["", "wrong", "A" * 64, True])
def test_additional_executable_expected_digest_must_be_canonical(tmp_path, digest):
    guard = custody(tmp_path)
    path = executable(tmp_path / "native-reference", "exit 0")
    with pytest.raises(tool_custody.ToolCustodyError, match="invalid expected"):
        guard.bind_executable(path, expected_sha256=digest)


def test_public_capture_keeps_healthy_epoch_when_file_access_updates_atime(tmp_path):
    path = executable(tmp_path / "version-tool", "printf 'version-tool 1.0\\n'")
    info = path.stat()
    os.utime(path, ns=(1_000_000_000, info.st_mtime_ns))
    before = path.stat()
    first = tool_custody.capture_executable(path)
    output = subprocess.check_output([path, "--version"], text=True)
    after = path.stat()
    assert output == "version-tool 1.0\n"
    assert after.st_atime_ns != before.st_atime_ns
    assert after.st_mtime_ns == before.st_mtime_ns
    assert after.st_ctime_ns == before.st_ctime_ns
    assert first == tool_custody.capture_executable(path)
    assert "atime_ns" not in first["stat"]
    first["sha256"] = "0" * 64
    assert tool_custody.capture_executable(path)["sha256"] != first["sha256"]


@pytest.mark.parametrize("path", [Path(""), Path("relative-tool")])
def test_public_capture_requires_absolute_path(path):
    with pytest.raises(tool_custody.ToolCustodyError, match="must be absolute"):
        tool_custody.capture_executable(path)
