"""Bind a proof runner's original outputs to the source that produced them."""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
from collections.abc import Iterable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from functools import cache
from pathlib import Path
from types import ModuleType
from typing import Any

from tools.benchmark.evidence import write_raw_file
from tools.ci.daemon_execution_evidence import NATIVE_RELEASE_SCOPES
from tools.ci.lint.handoff_validation import (
    _consume_repo_regular_file,
    _sha256_repo_regular_file,
)
from tools.ci.proof_json import parse_proof_json

P12A_PROOF = "p12a-proof-infrastructure"
BOUND_RUN_PROOFS = frozenset((*NATIVE_RELEASE_SCOPES.values(), P12A_PROOF))
GIT_REPOSITORY_OVERRIDES = (
    "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_NAMESPACE",
)


@dataclass(frozen=True)
class ProofRunSource:
    proof_id: str
    head: str
    dirty_digest: str
    cargo_lock_sha256: str | None

    def record(
        self, *, format: str, digests: dict[str, str], daemon_sha256: str | None = None,
    ) -> dict[str, Any]:
        return {
            "schema_version": 1,
            "proof_id": self.proof_id,
            "source": {"head": self.head, "dirty_digest": self.dirty_digest},
            "cargo_lock_sha256": self.cargo_lock_sha256,
            "daemon_sha256": daemon_sha256,
            "format": format,
            "artifacts": digests,
        }


@cache
def _source_checker() -> ModuleType:
    path = Path(__file__).with_name("lint") / "check-proof-authority.py"
    spec = importlib.util.spec_from_file_location("quanta_proof_run_source_checker", path)
    if spec is None or spec.loader is None:
        raise ValueError("proof source checker is unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def capture_run_source(
    root: Path, proof: dict[str, Any], *, outputs: Iterable[Path] = (),
) -> ProofRunSource:
    try:
        snapshot = _source_checker().proof_source_snapshot(
            root, manifest_path=root / proof["artifact"], proof=proof, excluded_paths=outputs,
        )
        cargo_lock = (
            _sha256_repo_regular_file(root, "Cargo.lock", label="proof Cargo.lock")
            if proof["binary_binding"] == "release-daemon"
            else None
        )
    except (OSError, RuntimeError, ValueError) as error:
        raise ValueError(f"cannot capture proof source: {error}") from error
    return ProofRunSource(proof["id"], snapshot["head"], snapshot["dirty_digest"], cargo_lock)


def require_run_source(
    root: Path, proof: dict[str, Any], captured: ProofRunSource, *,
    phase: str, outputs: Iterable[Path] = (),
) -> None:
    if capture_run_source(root, proof, outputs=outputs) != captured:
        raise ValueError(f"proof source changed during {phase}")


def _git(root: Path, *args: str, input_bytes: bytes | None = None) -> bytes:
    result = subprocess.run(
        ["git", "-C", str(root), *args], input=input_bytes, capture_output=True, check=False,
    )
    if result.returncode != 0:
        raise ValueError(f"cannot freeze proof source: {os.fsdecode(result.stderr).strip()}")
    return result.stdout


def _source_paths(root: Path) -> tuple[str, ...]:
    entries = _git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard")
    return tuple(sorted({os.fsdecode(path) for path in entries.split(b"\0") if path}))


def _file_versions(root: Path, paths: Iterable[str]) -> dict[str, tuple[int, ...] | None]:
    versions = {}
    for path in paths:
        try:
            info = (root / path).lstat()
        except FileNotFoundError:
            versions[path] = None  # A tracked deletion is part of the working tree.
        else:
            versions[path] = (
                info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns,
            )
    return versions


def _checkout_state(root: Path) -> tuple[bytes, bytes, dict[str, tuple[int, ...] | None]]:
    return (
        _git(root, "rev-parse", "HEAD"),
        _git(root, "ls-files", "--stage", "-z"),
        _file_versions(root, _source_paths(root)),
    )


def _copy_working_tree(
    root: Path, checkout: Path, versions: dict[str, tuple[int, ...] | None],
) -> None:
    for relative, version in versions.items():
        if version is None:
            continue
        destination = checkout / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        mode = version[2]
        if stat.S_ISLNK(mode):
            destination.symlink_to((root / relative).readlink())
        else:
            with destination.open("xb") as output:
                _consume_repo_regular_file(
                    root, relative, label="proof execution source",
                    consume=lambda source: shutil.copyfileobj(source, output),
                )
            destination.chmod(stat.S_IMODE(mode))


def _require_internal_links(root: Path, versions: dict[str, tuple[int, ...] | None]) -> None:
    for relative, version in versions.items():
        if version is None or not stat.S_ISLNK(version[2]):
            continue
        link = root / relative
        try:
            resolved = link.resolve()
        except RuntimeError as error:
            raise ValueError(f"cannot resolve proof source symlink: {relative}") from error
        if (
            link.readlink().is_absolute()
            or not resolved.is_relative_to(root)
            or resolved.is_relative_to(root / ".git")
        ):
            raise ValueError(f"proof source symlink leaves the working tree: {relative}")


def _seal_working_tree(checkout: Path) -> None:
    for directory, subdirectories, files in os.walk(checkout, topdown=True):
        root = Path(directory)
        if root == checkout:
            subdirectories.remove(".git")
        for name in files:
            path = root / name
            info = path.lstat()
            if not stat.S_ISLNK(info.st_mode):
                path.chmod(stat.S_IMODE(info.st_mode) & ~0o222)
        root.chmod(stat.S_IMODE(root.stat().st_mode) & ~0o222)


@dataclass(frozen=True)
class FrozenRunCheckout:
    root: Path
    head: bytes
    index: bytes
    versions: dict[str, tuple[int, ...] | None]

    def require_unchanged(self, phase: str) -> None:
        if _checkout_state(self.root) != (self.head, self.index, self.versions):
            raise ValueError(f"frozen proof checkout changed during {phase}")


@contextmanager
def frozen_run_checkout(
    root: Path, proof: dict[str, Any], captured: ProofRunSource, *,
    outputs: Iterable[Path] = (),
) -> Iterator[FrozenRunCheckout]:
    """Keep collection and execution on the same working-tree bytes and index."""
    outputs = tuple(outputs)
    if any(os.environ.get(name) for name in GIT_REPOSITORY_OVERRIDES):
        raise ValueError("proof execution requires Git to resolve the selected checkout directly")
    head, index, versions = _checkout_state(root)
    if any(entry.startswith(b"160000 ") for entry in index.split(b"\0")):
        raise ValueError("proof execution requires submodule source in the frozen checkout")
    _require_internal_links(root, versions)
    if os.statvfs(root).f_flag & os.ST_RDONLY:
        require_run_source(root, proof, captured, phase="checkout admission", outputs=outputs)
        yield FrozenRunCheckout(root, head, index, versions)
        return

    with tempfile.TemporaryDirectory(prefix="quanta-proof-source-") as temporary:
        checkout = Path(temporary).resolve()
        _git(checkout, "init", "-q", "--template=")
        objects = _git(root, "rev-parse", "--path-format=absolute", "--git-path", "objects")
        alternates = checkout / ".git" / "objects" / "info" / "alternates"
        alternates.write_bytes(objects)
        _git(checkout, "update-ref", "--no-deref", "HEAD", captured.head)
        _git(checkout, "update-index", "-z", "--index-info", input_bytes=index)
        _copy_working_tree(root, checkout, versions)
        _require_internal_links(checkout, versions)
        if _checkout_state(root) != (head, index, versions):
            raise ValueError("proof source changed while freezing the working tree")
        require_run_source(root, proof, captured, phase="checkout capture", outputs=outputs)
        cloned_outputs = tuple(
            checkout / path.relative_to(root) for path in outputs if path.is_relative_to(root)
        )
        require_run_source(
            checkout, proof, captured, phase="checkout materialization", outputs=cloned_outputs,
        )
        _seal_working_tree(checkout)
        yield FrozenRunCheckout(checkout, *_checkout_state(checkout))


def write_run_binding(
    path: Path, source: ProofRunSource, *, format: str, digests: dict[str, str],
    daemon_sha256: str | None = None,
) -> None:
    record = source.record(format=format, digests=digests, daemon_sha256=daemon_sha256)
    write_raw_file(path, [(json.dumps(record, sort_keys=True) + "\n").encode("utf-8")])


def verify_run_binding(
    content: bytes, source: ProofRunSource, *, format: str, digests: dict[str, str],
    daemon_sha256: str | None,
) -> None:
    record = parse_proof_json(content)
    expected = source.record(format=format, digests=digests, daemon_sha256=daemon_sha256)
    if (
        not isinstance(record, dict)
        or type(record.get("schema_version")) is not int
        or record != expected
    ):
        raise ValueError("captured execution source differs from the manifest or archived run")
