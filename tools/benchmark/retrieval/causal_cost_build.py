"""Observe a fresh scale build and retain local source/tool/binary custody.

The returned binding is produced in this invocation, never loaded from a caller's
receipt. It is local build evidence, not same-UID isolation or release attestation.
"""

from __future__ import annotations

import hashlib
import json
import os
import stat
import subprocess
from pathlib import Path
from typing import BinaryIO

from tools.benchmark.evidence import CONTROL_DOCUMENT_BYTES, RawFile
from tools.benchmark.retrieval.conditional_proof import canonical, sha
from tools.benchmark.retrieval.scanner_build_custody import EXECUTION_ENV_KEYS
from tools.benchmark.retrieval.scanner_source_identity import capture as source_capture
from tools.benchmark.retrieval.tool_custody import (
    ToolCustody,
    capture_executable,
    resolve_tool_paths,
    validate_environment,
)

BUILD_ARGS = (
    "--lane",
    "release-lane",
    "build",
    "-p",
    "quanta-index-searchd-harness",
    "--bin",
    "scale_matrix",
    "--all-features",
    "--locked",
    "--offline",
    "--release",
    "--message-format=json-render-diagnostics",
)
NATIVE_BUILD_ARGS = (
    "--lane",
    "bench-lane",
    "build",
    "-p",
    "quanta-index-searchd-harness",
    "--bin",
    "scale_matrix",
    "-p",
    "quanta-index-searchd-runtime",
    "--bin",
    "quanta-index-searchd",
    "--all-features",
    "--locked",
    "--offline",
    "--profile",
    "bench",
    "--message-format=json-render-diagnostics",
)

BACKENDS = ("legacy", "index-native")


def _environment(out: Path) -> dict[str, str]:
    validate_environment(os.environ)
    resource_wait = os.environ.get("QUANTA_INDEX_RESOURCE_WAIT_SECONDS")
    if resource_wait is not None:
        try:
            parsed_wait = int(resource_wait)
        except ValueError as error:
            raise ValueError("managed build resource wait must be a positive integer") from error
        if parsed_wait < 1:
            raise ValueError("managed build resource wait must be a positive integer")
    env = {key: value for key, value in os.environ.items() if key in EXECUTION_ENV_KEYS}
    if not env.get("HOME") or not env.get("PATH"):
        raise ValueError("managed build requires HOME and PATH")
    env.update(
        CARGO_BUILD_JOBS="1",
        CARGO_TARGET_DIR=str(out / "target"),
        QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR="1",
        QUANTA_INDEX_SCCACHE="0",
        QUANTA_INDEX_TARGET_GC="0",
        QUANTA_INDEX_RESOURCE_ADMISSION="auto",
        QUANTA_INDEX_BUILD_LOGGING="0",
        CARGO_NET_OFFLINE="true",
        RUSTC_WRAPPER="",
        RUSTC_WORKSPACE_WRAPPER="",
        PYTHONDONTWRITEBYTECODE="1",
    )
    return env


def _configs(cwd: Path, env: dict[str, str]) -> dict[str, str]:
    directories = [cwd, *cwd.parents]
    cargo_home = Path(env.get("CARGO_HOME", str(Path(env["HOME"]) / ".cargo")))
    paths = {
        directory / ".cargo" / name
        for directory in directories
        for name in ("config", "config.toml")
    }
    paths.update(cargo_home / name for name in ("config", "config.toml"))
    return {str(path): RawFile.capture(path).sha256 for path in sorted(paths) if path.exists()}


def _dependency_inputs(metadata: dict, cwd: Path) -> dict[str, str]:
    """Bind external dependency source bytes; internal packages are in source_capture."""
    result = {}
    for package in metadata["packages"]:
        root = Path(package["manifest_path"]).parent.resolve(strict=True)
        if root == cwd or cwd in root.parents:
            continue
        if package.get("source") is None:
            raise ValueError(f"managed build refuses external path dependency: {root}")
        inventory = []
        for path in sorted(root.rglob("*")):
            mode = path.lstat().st_mode
            if stat.S_ISDIR(mode):
                continue
            if not stat.S_ISREG(mode):
                raise ValueError(f"dependency input is not a regular file: {path}")
            inventory.append([path.relative_to(root).as_posix(), RawFile.capture(path).sha256])
        result[str(root)] = sha(canonical(inventory))
    return result


def _artifacts(stdout: BinaryIO, cwd: Path, out: Path, *, backend: str) -> dict[str, Path]:
    if backend not in BACKENDS:
        raise ValueError(f"unknown managed build backend: {backend}")
    expected = {
        "scale_matrix": cwd / "crates/quanta-index-searchd-harness/Cargo.toml",
    }
    if backend == "index-native":
        expected["quanta-index-searchd"] = cwd / "crates/quanta-index-searchd-runtime/Cargo.toml"
    finished = []
    artifacts: dict[str, Path] = {}
    last = None
    while line := stdout.readline(CONTROL_DOCUMENT_BYTES + 1):
        if finished:
            raise ValueError("managed build has output after Cargo terminal result")
        if len(line) > CONTROL_DOCUMENT_BYTES:
            raise ValueError("managed build Cargo row exceeds control-document admission")
        try:
            row = json.loads(line)
        except ValueError as error:
            raise ValueError("managed build Cargo stdout contains a non-JSON row") from error
        if not isinstance(row, dict):
            raise ValueError("managed build Cargo row is not an object")
        last = row
        if row.get("reason") == "build-finished":
            finished.append(row)
            if len(finished) > 1:
                raise ValueError("managed build has repeated Cargo terminal results")
        if row.get("reason") != "compiler-artifact":
            continue
        target_row = row.get("target")
        if not isinstance(target_row, dict):
            raise ValueError("managed build Cargo artifact target is malformed")
        name = target_row.get("name")
        if not isinstance(name, str):
            raise ValueError("managed build Cargo artifact name is malformed")
        if name not in expected:
            continue
        if name in artifacts:
            raise ValueError(f"managed build has repeated {name} artifacts")
        profile = row.get("profile")
        if (
            target_row.get("kind") != ["bin"]
            or row.get("manifest_path") != str(expected[name])
            or not isinstance(profile, dict)
            or profile.get("test") is not False
            or row.get("fresh") is not False
            or not isinstance(row.get("executable"), str)
        ):
            raise ValueError(f"managed build requires a fresh non-test {name} from its manifest")
        executable = Path(row["executable"])
        if not executable.is_absolute() or executable.is_symlink():
            raise ValueError("managed build artifact path is invalid")
        try:
            executable = executable.resolve(strict=True)
            target_root = (out / "target").resolve(strict=True)
        except OSError as error:
            raise ValueError("managed build artifact or fresh target is absent") from error
        if target_root not in executable.parents:
            raise ValueError("managed build artifact escapes the fresh target")
        if backend == "index-native" and (
            executable.parent != target_root / "release" or executable.name != name
        ):
            raise ValueError(f"managed build {name} is not the bench-profile binary")
        artifacts[name] = executable
    if len(finished) != 1 or finished[0].get("success") is not True or last != finished[0]:
        raise ValueError("managed build lacks one final successful Cargo result")
    if set(artifacts) != set(expected):
        missing = sorted(set(expected) - set(artifacts))
        raise ValueError(f"managed build lacks expected artifacts: {missing}")
    return artifacts


def _artifact(stdout: BinaryIO, cwd: Path, out: Path) -> Path:
    """Keep the legacy single-binary parser entry for existing consumers."""
    return _artifacts(stdout, cwd, out, backend="legacy")["scale_matrix"]


def _copy_artifact(artifact: Path, out: Path, name: str) -> tuple[Path, dict]:
    epoch = capture_executable(artifact)
    artifact_input = RawFile.capture(artifact)
    binary = out / name
    with binary.open("xb") as destination:
        artifact_input.consume_seekable(lambda source: _copy(source, destination))
    binary.chmod(0o500)
    copied = capture_executable(binary)
    if copied["sha256"] != epoch["sha256"] or capture_executable(artifact) != epoch:
        raise ValueError("managed artifact changed while copying")
    return binary, copied


def build_scale(cwd: Path, out_root: Path, *, backend: str = "legacy") -> dict:
    """Build once through cargow with fresh artifacts; preserve raw output on failure."""
    if backend not in BACKENDS:
        raise ValueError(f"unknown managed build backend: {backend}")
    cwd = cwd.resolve(strict=True)
    out = Path(os.path.realpath(out_root))
    if out == cwd or cwd in out.parents or out.exists():
        raise ValueError("managed build output must be new and outside the checkout")
    env = _environment(out)
    paths = resolve_tool_paths(cwd, env)
    head_result = subprocess.run(
        [str(paths["git"]), "-C", str(cwd), "rev-parse", "HEAD"],
        env=env,
        capture_output=True,
        check=True,
        timeout=30,
    )
    head = head_result.stdout.decode().strip()
    before = source_capture(cwd, head, env=env)
    out.mkdir(mode=0o700, parents=False)
    paths["cargow"] = cwd / "scripts/cargow"
    tools = {
        name: {
            "path": str(path),
            "realpath": epoch["realpath"],
            "sha256": epoch["sha256"],
            "version": "executable epoch bound",
        }
        for name, path in paths.items()
        for epoch in [capture_executable(path)]
    }
    guard = ToolCustody.create(cwd, out / "bin", tools=tools, environment=env)
    env = guard.environment()
    configs = _configs(cwd, env)
    metadata_result = subprocess.run(
        [
            str(paths["cargow"]),
            "--lane",
            "release-lane",
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
            "--offline",
        ],
        cwd=cwd,
        env=env,
        capture_output=True,
        check=True,
    )
    metadata = json.loads(metadata_result.stdout)
    dependencies = _dependency_inputs(metadata, cwd)
    guard.check()
    if source_capture(cwd, head, env=env) != before:
        raise ValueError("source changed while preparing managed build")
    command = [str(paths["cargow"]), *(BUILD_ARGS if backend == "legacy" else NATIVE_BUILD_ARGS)]
    with (out / "build.stdout").open("xb") as stdout, (out / "build.stderr").open("xb") as stderr:
        result = subprocess.run(
            command, cwd=cwd, env=env, stdout=stdout, stderr=stderr, check=False
        )
    guard.check()
    if (
        source_capture(cwd, head, env=env) != before
        or _configs(cwd, env) != configs
        or _dependency_inputs(metadata, cwd) != dependencies
    ):
        raise ValueError("source, configuration or dependency bytes changed during managed build")
    if result.returncode:
        raise ValueError(f"managed build failed with exit {result.returncode}; raw output retained")
    stdout_input = RawFile.capture(out / "build.stdout")
    artifacts = stdout_input.consume_seekable(
        lambda stream: _artifacts(stream, cwd, out, backend=backend)
    )
    binary, copied = _copy_artifact(artifacts["scale_matrix"], out, "scale_matrix")
    native_daemon = None
    if backend == "index-native":
        daemon_binary, daemon_epoch = _copy_artifact(
            artifacts["quanta-index-searchd"], out, "quanta-index-searchd"
        )
        native_daemon = {
            "binary": str(daemon_binary),
            "binary_sha256": daemon_epoch["sha256"],
            "binary_epoch": daemon_epoch,
        }
    receipt = {
        "schema_version": 1,
        "status": "OBSERVED_LOCAL_BUILD",
        "source_revision": head,
        "source_identity": before,
        "binary": str(binary),
        "binary_sha256": copied["sha256"],
        "binary_epoch": copied,
        "command": command,
        "build_exit_code": 0,
        "build_stdout_sha256": stdout_input.sha256,
        "build_stderr_sha256": RawFile.capture(out / "build.stderr").sha256,
        "dependencies": dependencies,
        "cargo_configs": configs,
        "tools": guard.record(),
        "environment_sha256": {
            key: hashlib.sha256(value.encode()).hexdigest() for key, value in sorted(env.items())
        },
        "scope": "observed fresh local build; excludes same-UID/kernel isolation and compiler/system libraries",
    }
    if backend == "index-native":
        receipt.update(
            backend=backend,
            build_profile="bench",
            native_daemon=native_daemon,
        )
    with (out / "build-receipt.json").open("xb") as stream:
        stream.write(canonical(receipt) + b"\n")
    return receipt


def _copy(source, destination) -> None:
    while chunk := source.read(1024 * 1024):
        destination.write(chunk)
