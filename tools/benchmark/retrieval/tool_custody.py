"""Pin local invocation tools and refuse epoch drift around producer execution.

This is local file custody, not same-UID/kernel isolation, trusted remote
producer attestation, or compiler-library/system-dependency attestation.
Offline record validation does not require the producer machine's paths.
"""

from __future__ import annotations

import copy
import hashlib
import os
import shlex
import shutil
import stat
import subprocess
import sys
from collections.abc import Mapping
from pathlib import Path

TOOL_NAMES = ("python", "cargo", "rustc", "cargo-nextest", "git", "bash", "just")


class ToolCustodyError(ValueError):
    """A selected executable or its invocation alias changed during custody."""


def validate_environment(environment: Mapping[str, str]) -> None:
    """Refuse inherited Bash execution hooks before any tool invocation.

    A function import can override PATH-selected commands or shell builtins;
    startup files can introduce those functions even when none was exported.
    Reject the entire export namespace, including empty or malformed entries.
    """
    functions = sorted(key for key in environment if key.startswith("BASH_FUNC_"))
    if functions:
        raise ToolCustodyError("inherited Bash function exports are not admitted: " + ", ".join(functions))
    for name in ("ENV", "BASH_ENV"):
        if environment.get(name):
            raise ToolCustodyError(f"inherited shell startup setting is not admitted: {name}")


def _stat(value: os.stat_result) -> dict[str, int]:
    return {name: getattr(value, "st_" + name) for name in
            ("dev", "ino", "mode", "size", "mtime_ns", "ctime_ns")}


def _chain(path: Path) -> list[dict]:
    """Bind symlink metadata and directory identities without directory churn."""
    entries = []
    for prefix in reversed((path, *path.parents)):
        info = prefix.lstat()
        entry = {"path": str(prefix), "dev": info.st_dev, "ino": info.st_ino,
                 "mode": info.st_mode}
        if stat.S_ISLNK(info.st_mode):
            entry.update(target=os.readlink(prefix), ctime_ns=info.st_ctime_ns,
                         mtime_ns=info.st_mtime_ns, size=info.st_size)
        entries.append(entry)
    return entries


def _epoch(invocation: Path) -> dict:
    invocation = invocation.absolute()
    before_chain = _chain(invocation)
    resolved = invocation.resolve(strict=True)
    resolved_chain = _chain(resolved)
    descriptor = os.open(resolved, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode) or not before.st_mode & 0o111:
            raise ToolCustodyError(f"selected tool is not an executable regular file: {resolved}")
        digest = hashlib.sha256()
        remaining = before.st_size
        while remaining:
            chunk = stream.read(min(1024 * 1024, remaining))
            if not chunk:
                raise ToolCustodyError(f"selected tool was truncated while reading: {invocation}")
            digest.update(chunk)
            remaining -= len(chunk)
        after = os.fstat(stream.fileno())
    if (_stat(before) != _stat(after) or _stat(resolved.lstat()) != _stat(before)
            or before_chain != _chain(invocation) or resolved_chain != _chain(resolved)):
        raise ToolCustodyError(f"selected tool changed while reading: {invocation}")
    return {"invocation": str(invocation), "realpath": str(resolved),
            "sha256": digest.hexdigest(), "stat": _stat(before),
            "invocation_chain": before_chain, "resolved_chain": resolved_chain}


def capture_executable(path: Path) -> dict:
    """Capture the canonical epoch of an absolute executable invocation."""
    if not isinstance(path, Path) or not path.is_absolute():
        raise ToolCustodyError("executable path must be absolute")
    try:
        return copy.deepcopy(_epoch(path))
    except (OSError, ValueError) as error:
        raise ToolCustodyError(f"executable unavailable: {path}: {error}") from error


def resolve_tool_paths(root: Path, environment: Mapping[str, str]) -> dict[str, Path]:
    """Select real Rust binaries using rustup in the requested source context."""
    validate_environment(environment)
    if os.name == "nt":
        raise ToolCustodyError("tool custody requires Unix Bash invocation support")
    env = dict(environment)
    rustup_alias = shutil.which("rustup", path=env.get("PATH", os.defpath))
    if rustup_alias is None:
        raise ToolCustodyError("required executable unavailable: rustup")
    rustup = Path(rustup_alias).absolute()
    selector_epoch = _epoch(rustup)
    paths = {"python": Path(sys.executable).absolute()}
    for name in ("cargo", "rustc"):
        result = subprocess.run([selector_epoch["realpath"], "which", name], cwd=root,
                                env=env, capture_output=True, check=False, timeout=30)
        if result.returncode:
            raise ToolCustodyError(f"rustup could not select {name}: {result.stderr.decode(errors='replace')}")
        selected = result.stdout.decode("utf-8").strip()
        if not selected or "\n" in selected or not Path(selected).is_absolute():
            raise ToolCustodyError(f"rustup selected malformed {name} path")
        paths[name] = Path(_epoch(Path(selected))["realpath"])
    if _epoch(rustup) != selector_epoch:
        raise ToolCustodyError("rustup selector changed during tool selection")
    for name in ("cargo-nextest", "git", "bash", "just"):
        selected = shutil.which(name, path=env.get("PATH", os.defpath))
        if selected is None:
            raise ToolCustodyError(f"required executable unavailable: {name}")
        paths[name] = Path(_epoch(Path(selected))["realpath"])
    return paths


class ToolCustody:
    """An invocation epoch; call check immediately before and after each execute."""

    def __init__(self, root: Path, bin_dir: Path, tools: dict, environment: dict,
                 epochs: dict):
        self.root, self.bin_dir = root, bin_dir
        self._tools, self._environment, self._epochs = tools, environment, epochs

    @classmethod
    def create(cls, root: Path, bin_dir: Path, *, tools: Mapping[str, Mapping[str, str]],
               environment: Mapping[str, str] | None = None) -> ToolCustody:
        env = dict(os.environ if environment is None else environment)
        validate_environment(env)
        rows = copy.deepcopy(dict(tools))
        if set(rows) != {*TOOL_NAMES, "cargow"}:
            raise ToolCustodyError("tool metadata must name every canonical tool and cargow")
        for name, row in rows.items():
            if set(row) != {"path", "realpath", "sha256", "version"}:
                raise ToolCustodyError(f"invalid selected tool metadata: {name}")
            epoch = _epoch(Path(row["path"]))
            if row["realpath"] != epoch["realpath"] or row["sha256"] != epoch["sha256"]:
                raise ToolCustodyError(f"selected tool metadata differs from local file: {name}")
        bin_dir = bin_dir.absolute()
        bin_dir.mkdir(mode=0o700)
        os.chmod(bin_dir, 0o700)
        epochs = {}
        for name, row in rows.items():
            original = Path(row["path"]).absolute()
            epochs[str(original)] = _epoch(original)
            if name == "cargow":
                continue
            alias = bin_dir / ("python3" if name == "python" else name)
            if name == "python":
                # Executing a symlink to the resolved base interpreter loses the
                # original venv. Invoke its original absolute alias instead.
                content = (f"#!{rows['bash']['realpath']}\n"
                           f"exec {shlex.quote(str(original))} \"$@\"\n").encode()
                with alias.open("xb") as stream:
                    stream.write(content)
                    stream.flush()
                    os.fsync(stream.fileno())
                alias.chmod(0o700)
            else:
                alias.symlink_to(row["realpath"])
                row["path"] = str(alias)
            epochs[str(alias)] = _epoch(alias)
        env.update(PATH=str(bin_dir) + os.pathsep + env.get("PATH", os.defpath),
                   RUSTC=rows["rustc"]["realpath"], RUSTC_WRAPPER="",
                   RUSTC_WORKSPACE_WRAPPER="", QUANTA_INDEX_SCCACHE="0")
        instance = cls(root.absolute(), bin_dir, rows, env, epochs)
        instance.check()
        return instance

    def environment(self) -> dict[str, str]:
        return dict(self._environment)

    def tools(self) -> dict[str, dict[str, str]]:
        return copy.deepcopy(self._tools)

    def bind_executable(self, path: Path, *, expected_sha256: str | None = None) -> dict:
        """Bind an additional absolute executable or recheck its existing epoch."""
        if not isinstance(path, Path) or not path.is_absolute():
            raise ToolCustodyError("additional executable path must be absolute")
        if expected_sha256 is not None and (
            not isinstance(expected_sha256, str) or len(expected_sha256) != 64
            or any(c not in "0123456789abcdef" for c in expected_sha256)
        ):
            raise ToolCustodyError("invalid expected executable digest")
        try:
            actual = _epoch(path)
        except (OSError, ValueError) as error:
            raise ToolCustodyError(f"additional executable unavailable: {path}: {error}") from error
        if expected_sha256 is not None and actual["sha256"] != expected_sha256:
            raise ToolCustodyError(f"additional executable digest differs: {path}")
        key = actual["invocation"]
        if key in self._epochs and self._epochs[key] != actual:
            raise ToolCustodyError(f"tool custody epoch changed: {path}")
        self._epochs[key] = actual
        return copy.deepcopy(actual)

    def record(self) -> dict:
        return {"schema_version": 1, "tools": self.tools(), "epochs": copy.deepcopy(self._epochs),
                "private_bin": str(self.bin_dir),
                "scope": "local invocation custody; excludes same-UID/kernel isolation and compiler/system libraries"}

    def check(self) -> None:
        for path, expected in self._epochs.items():
            try:
                actual = _epoch(Path(path))
            except (OSError, ValueError) as error:
                raise ToolCustodyError(f"tool custody unavailable: {path}: {error}") from error
            if actual != expected:
                raise ToolCustodyError(f"tool custody epoch changed: {path}")


def validate_record(value: object) -> None:
    """Validate archived custody structure without opening producer-host paths."""
    if not isinstance(value, dict) or set(value) != {
        "schema_version", "tools", "epochs", "private_bin", "scope"
    } or type(value["schema_version"]) is not int or value["schema_version"] != 1:
        raise ToolCustodyError("invalid archived tool custody envelope")
    if not isinstance(value["tools"], dict) or set(value["tools"]) != {*TOOL_NAMES, "cargow"}:
        raise ToolCustodyError("invalid archived tool inventory")
    if not isinstance(value["epochs"], dict) or not value["epochs"]:
        raise ToolCustodyError("missing archived tool epochs")
    if not isinstance(value["private_bin"], str) or not Path(value["private_bin"]).is_absolute():
        raise ToolCustodyError("invalid archived private tool directory")
    if not isinstance(value["scope"], str) or not value["scope"]:
        raise ToolCustodyError("missing archived custody scope")
    aliases = {str(Path(value["private_bin"]) / name) for name in
               ("python3", "cargo", "rustc", "cargo-nextest", "git", "bash", "just")}
    if not aliases <= set(value["epochs"]):
        raise ToolCustodyError("missing archived private invocation aliases")
    for row in value["tools"].values():
        if not isinstance(row, dict) or set(row) != {"path", "realpath", "sha256", "version"}:
            raise ToolCustodyError("invalid archived tool metadata")
        if any(not isinstance(row[key], str) or not row[key] for key in row):
            raise ToolCustodyError("empty archived tool metadata")
        if len(row["sha256"]) != 64 or any(c not in "0123456789abcdef" for c in row["sha256"]):
            raise ToolCustodyError("invalid archived tool digest")
        if not Path(row["path"]).is_absolute() or not Path(row["realpath"]).is_absolute():
            raise ToolCustodyError("archived tool paths must be absolute references")
        epoch = value["epochs"].get(row["path"])
        if not isinstance(epoch, dict) or epoch.get("realpath") != row["realpath"] or epoch.get("sha256") != row["sha256"]:
            raise ToolCustodyError("archived invocation differs from selected tool")
    for path, epoch in value["epochs"].items():
        if not isinstance(path, str) or not Path(path).is_absolute() or not isinstance(epoch, dict):
            raise ToolCustodyError("invalid archived epoch path")
        if set(epoch) != {"invocation", "realpath", "sha256", "stat", "invocation_chain", "resolved_chain"} or epoch["invocation"] != path:
            raise ToolCustodyError("invalid archived epoch fields")
        if not isinstance(epoch["realpath"], str) or not Path(epoch["realpath"]).is_absolute():
            raise ToolCustodyError("invalid archived resolved path")
        if not isinstance(epoch["sha256"], str) or len(epoch["sha256"]) != 64 or any(c not in "0123456789abcdef" for c in epoch["sha256"]):
            raise ToolCustodyError("invalid archived epoch digest")
        if not isinstance(epoch["stat"], dict) or set(epoch["stat"]) != {"dev", "ino", "mode", "size", "mtime_ns", "ctime_ns"}:
            raise ToolCustodyError("invalid archived tool stat")
        if any(type(v) is not int or v < 0 for v in epoch["stat"].values()):
            raise ToolCustodyError("invalid archived tool stat scalar")
        if not stat.S_ISREG(epoch["stat"]["mode"]) or not epoch["stat"]["mode"] & 0o111:
            raise ToolCustodyError("archived tool must be an executable regular file")
        for key in ("invocation_chain", "resolved_chain"):
            if not isinstance(epoch[key], list) or not epoch[key]:
                raise ToolCustodyError("missing archived alias chain")
            target = Path(path if key == "invocation_chain" else epoch["realpath"])
            expected_paths = [str(p) for p in reversed((target, *target.parents))]
            if len(epoch[key]) != len(expected_paths):
                raise ToolCustodyError("incomplete archived alias chain")
            for entry, expected_path in zip(epoch[key], expected_paths):
                if not isinstance(entry, dict) or entry.get("path") != expected_path:
                    raise ToolCustodyError("invalid archived alias chain identity")
                required = {"path", "dev", "ino", "mode"}
                if type(entry.get("mode")) is not int:
                    raise ToolCustodyError("invalid archived alias mode")
                if stat.S_ISLNK(entry["mode"]):
                    required |= {"target", "ctime_ns", "mtime_ns", "size"}
                    if not isinstance(entry.get("target"), str) or not entry["target"]:
                        raise ToolCustodyError("invalid archived symlink target")
                if set(entry) != required or any(type(entry[k]) is not int or entry[k] < 0
                                                  for k in required - {"path", "target"}):
                    raise ToolCustodyError("invalid archived alias metadata")
