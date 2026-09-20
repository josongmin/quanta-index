#!/usr/bin/env python3
"""Validate SEP-21 proof authority and source-bound proof manifests."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Any

import jsonschema

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[3]
REGISTRY_PATH = ROOT / "tools/ci/proof-authority.toml"
SCHEMA_PATH = ROOT / "tools/ci/proof-manifest.schema.json"
FAMILIES = frozenset("SUADPFQX")
CHECKPOINTS = ("M0", "M1", "M2", "M3", "M4", "M5")
GATES = frozenset(("pr", "merge", "correctness", "release"))
PROOF_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]+$")
TICKET_RE = re.compile(r"^S21-(?:0[0-9]|1[0-3])$")
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
PAIRED_REPOSITORY = "github:josongmin/semantica-codegraph-v2"
PAIRED_DEPENDENCY_LOCK = "Cargo.lock"


@dataclass(frozen=True)
class Finding:
    path: Path
    message: str

    def render(self) -> str:
        return f"{self.path}: {self.message}"


def _read_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        value = tomllib.load(handle)
    if not isinstance(value, dict):
        raise ValueError("registry root is not a table")
    return value


def _read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def check_registry(data: dict[str, Any], *, root: Path, path: Path) -> list[Finding]:
    findings: list[Finding] = []
    if data.get("schema") != 1:
        findings.append(Finding(path, "`schema` must be 1"))

    families = data.get("families")
    if not isinstance(families, dict):
        findings.append(Finding(path, "`families` must be a table"))
        families = {}
    family_ids = set(families)
    if family_ids != FAMILIES:
        findings.append(
            Finding(path, f"families must be exactly {sorted(FAMILIES)}, got {sorted(family_ids)}")
        )
    for family_id, family in families.items():
        if not isinstance(family, dict):
            findings.append(Finding(path, f"family {family_id!r} is not a table"))
            continue
        for field in ("name", "description"):
            if not isinstance(family.get(field), str) or not family[field].strip():
                findings.append(Finding(path, f"family {family_id!r} has no non-empty {field}"))

    proofs = data.get("proofs")
    if not isinstance(proofs, list) or not proofs:
        findings.append(Finding(path, "`proofs` must be a non-empty array of tables"))
        return findings
    test_authority_path = root / "tools/ci/test-authority.toml"
    try:
        test_authority = _read_toml(test_authority_path)
        known_test_targets = {
            target.get("id")
            for section in ("integration_targets", "fuzz_targets")
            for target in test_authority.get(section, [])
            if isinstance(target, dict) and isinstance(target.get("id"), str)
        }
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        findings.append(Finding(test_authority_path, f"cannot load test authority: {error}"))
        known_test_targets = set()

    seen_ids: set[str] = set()
    seen_artifacts: set[str] = set()
    tickets: set[str] = set()
    covered_families: set[str] = set()
    for index, proof in enumerate(proofs):
        where = f"proofs[{index}]"
        if not isinstance(proof, dict):
            findings.append(Finding(path, f"{where} is not a table"))
            continue
        for field in (
            "id",
            "ticket",
            "family",
            "checkpoint",
            "owner",
            "command",
            "gate",
            "profile",
            "target",
            "filter",
            "source_binding",
            "binary_binding",
            "artifact_schema",
            "artifact",
            "required_host",
        ):
            if not isinstance(proof.get(field), str) or not proof[field].strip():
                findings.append(Finding(path, f"{where}.{field} must be a non-empty string"))
        proof_id = proof.get("id")
        if isinstance(proof_id, str):
            if not PROOF_ID_RE.fullmatch(proof_id):
                findings.append(
                    Finding(path, f"{where}.id {proof_id!r} is not canonical kebab-case")
                )
            if proof_id in seen_ids:
                findings.append(Finding(path, f"duplicate proof id {proof_id!r}"))
            seen_ids.add(proof_id)
        ticket = proof.get("ticket")
        if isinstance(ticket, str):
            if not TICKET_RE.fullmatch(ticket):
                findings.append(Finding(path, f"{where}.ticket {ticket!r} is not S21-00..S21-13"))
            tickets.add(ticket)
        family = proof.get("family")
        if isinstance(family, str):
            if family not in FAMILIES:
                findings.append(Finding(path, f"{where}.family {family!r} is not registered"))
            covered_families.add(family)
        checkpoint = proof.get("checkpoint")
        if isinstance(checkpoint, str) and checkpoint not in CHECKPOINTS:
            findings.append(Finding(path, f"{where}.checkpoint {checkpoint!r} is not M0..M5"))
        gate = proof.get("gate")
        if isinstance(gate, str) and gate not in GATES:
            findings.append(Finding(path, f"{where}.gate {gate!r} is not registered"))
        source_binding = proof.get("source_binding")
        if isinstance(source_binding, str) and source_binding not in {"exact", "exact-pair"}:
            findings.append(Finding(path, f"{where}.source_binding {source_binding!r} is invalid"))
        if source_binding == "exact":
            for field in ("paired_repository", "paired_dependency_lock"):
                if field in proof:
                    findings.append(
                        Finding(path, f"{where}.{field} is forbidden for exact source binding")
                    )
        elif source_binding == "exact-pair":
            if proof.get("paired_repository") != PAIRED_REPOSITORY:
                findings.append(
                    Finding(
                        path,
                        f"{where}.paired_repository must be {PAIRED_REPOSITORY!r}",
                    )
                )
            if proof.get("paired_dependency_lock") != PAIRED_DEPENDENCY_LOCK:
                findings.append(
                    Finding(
                        path,
                        f"{where}.paired_dependency_lock must be {PAIRED_DEPENDENCY_LOCK!r}",
                    )
                )
        binary_binding = proof.get("binary_binding")
        if isinstance(binary_binding, str) and binary_binding not in {"none", "release-daemon"}:
            findings.append(Finding(path, f"{where}.binary_binding {binary_binding!r} is invalid"))
        if family in {"P", "Q", "X"} and proof.get("required_host") != "linux-production-like":
            findings.append(
                Finding(path, f"{where} family {family} requires linux-production-like host")
            )
        if family in {"P", "X"} and binary_binding != "release-daemon":
            findings.append(
                Finding(path, f"{where} family {family} requires release-daemon binding")
            )
        artifact_schema = proof.get("artifact_schema")
        if isinstance(artifact_schema, str) and not (root / artifact_schema).is_file():
            findings.append(
                Finding(path, f"{where}.artifact_schema does not exist: {artifact_schema}")
            )
        owner = proof.get("owner")
        if isinstance(owner, str) and owner and not (root / owner).is_file():
            findings.append(Finding(path, f"{where}.owner does not exist: {owner}"))
        artifact = proof.get("artifact")
        if isinstance(artifact, str):
            artifact_path = Path(artifact)
            if artifact_path.is_absolute() or ".." in artifact_path.parts:
                findings.append(
                    Finding(path, f"{where}.artifact must be repo-relative: {artifact}")
                )
            if artifact in seen_artifacts:
                findings.append(Finding(path, f"duplicate artifact path {artifact!r}"))
            seen_artifacts.add(artifact)
        test_targets = proof.get("test_authority_targets")
        if not isinstance(test_targets, list) or any(
            not isinstance(item, str) for item in test_targets
        ):
            findings.append(
                Finding(path, f"{where}.test_authority_targets must be an array of strings")
            )
        else:
            for target_id in test_targets:
                if target_id not in known_test_targets:
                    findings.append(
                        Finding(
                            path,
                            f"{where} names unknown test-authority target {target_id!r}",
                        )
                    )
        dependencies = proof.get("dependencies")
        if not isinstance(dependencies, list) or any(
            not isinstance(item, str) for item in dependencies
        ):
            findings.append(Finding(path, f"{where}.dependencies must be an array of proof IDs"))

    expected_tickets = {f"S21-{index:02d}" for index in range(14)}
    if tickets != expected_tickets:
        findings.append(
            Finding(
                path,
                f"proof tickets must cover S21-00..S21-13 exactly; missing={sorted(expected_tickets - tickets)} extra={sorted(tickets - expected_tickets)}",
            )
        )
    if covered_families != FAMILIES:
        findings.append(
            Finding(
                path,
                f"proofs must cover every family; missing={sorted(FAMILIES - covered_families)}",
            )
        )
    proof_by_id = {
        proof["id"]: proof
        for proof in proofs
        if isinstance(proof, dict) and isinstance(proof.get("id"), str)
    }
    for proof_id, proof in proof_by_id.items():
        for dependency in proof.get("dependencies", []):
            if dependency not in proof_by_id:
                findings.append(
                    Finding(path, f"proof {proof_id!r} depends on unknown proof {dependency!r}")
                )

    visiting: set[str] = set()
    visited: set[str] = set()

    def visit(proof_id: str) -> None:
        if proof_id in visited:
            return
        if proof_id in visiting:
            findings.append(Finding(path, f"proof dependency cycle reaches {proof_id!r}"))
            return
        visiting.add(proof_id)
        for dependency in proof_by_id[proof_id].get("dependencies", []):
            if dependency in proof_by_id:
                visit(dependency)
        visiting.remove(proof_id)
        visited.add(proof_id)

    for proof_id in proof_by_id:
        visit(proof_id)
    return findings


def _parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _payload_repo_file(
    root: Path,
    value: str,
    *,
    label: str,
) -> tuple[Path | None, str | None]:
    """Resolve an untrusted manifest path without permitting repository escape."""

    relative = PurePosixPath(value)
    if (
        not value
        or "\\" in value
        or relative.is_absolute()
        or ".." in relative.parts
        or relative.as_posix() != value
        or relative == PurePosixPath(".")
    ):
        return None, f"{label} path must be canonical repo-relative: {value!r}"
    try:
        resolved_root = root.resolve()
        resolved = (resolved_root / Path(*relative.parts)).resolve()
    except (OSError, RuntimeError) as error:
        return None, f"{label} path cannot be resolved safely: {value!r}: {error}"
    try:
        resolved.relative_to(resolved_root)
    except ValueError:
        return None, f"{label} path escapes repository root: {value!r}"
    return resolved, None


def _git(root: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(root), *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or f"git {' '.join(args)} failed")
    return completed.stdout.strip()


def _git_bytes(root: Path, *args: str) -> bytes:
    completed = subprocess.run(
        ["git", "-C", str(root), *args],
        check=False,
        capture_output=True,
    )
    if completed.returncode != 0:
        message = os.fsdecode(completed.stderr).strip()
        raise RuntimeError(message or f"git {' '.join(args)} failed")
    return completed.stdout


def _optional_git(root: Path, *args: str) -> str | None:
    completed = subprocess.run(
        ["git", "-C", str(root), *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode == 0:
        return completed.stdout.strip()
    return None


def _digest_record(digest: Any, *fields: bytes) -> None:
    for field in fields:
        digest.update(len(field).to_bytes(8, "big"))
        digest.update(field)


def _repo_relative_bytes(root: Path, path: Path) -> bytes | None:
    candidate = path if path.is_absolute() else root / path
    try:
        relative = candidate.absolute().relative_to(root.absolute())
    except ValueError:
        return None
    raw = os.fsencode(relative)
    return None if raw in {b"", b"."} else raw


def _is_excluded(raw_path: bytes, excluded_paths: tuple[bytes, ...]) -> bool:
    return any(raw_path == item or raw_path.startswith(item + b"/") for item in excluded_paths)


def _working_tree_bytes(root: Path, raw_path: bytes) -> tuple[bytes, bytes]:
    native_path = os.fsencode(root) + b"/" + raw_path
    try:
        metadata = os.lstat(native_path)
    except FileNotFoundError:
        return b"missing", b""
    mode = f"{stat.S_IMODE(metadata.st_mode):04o}".encode()
    if stat.S_ISLNK(metadata.st_mode):
        return b"symlink:" + mode, os.fsencode(os.readlink(native_path))
    if stat.S_ISREG(metadata.st_mode):
        with open(native_path, "rb") as handle:
            return b"file:" + mode, handle.read()
    if stat.S_ISDIR(metadata.st_mode):
        # A changed submodule is a source-state change, but its contents are a
        # separate repository. Bind its exact checked-out commit and dirty bit.
        submodule = root / os.fsdecode(raw_path)
        head = _optional_git(submodule, "rev-parse", "--verify", "HEAD") or "missing"
        dirty = _git_bytes(submodule, "status", "--porcelain=v1", "-z")
        return b"directory:" + mode, head.encode() + b"\0" + dirty
    return b"special:" + mode, b""


def _index_entries(root: Path, raw_path: bytes) -> list[tuple[bytes, bytes]]:
    output = _git_bytes(root, "ls-files", "--stage", "-z", "--", os.fsdecode(raw_path))
    entries: list[tuple[bytes, bytes]] = []
    for raw_entry in (entry for entry in output.split(b"\0") if entry):
        header, _, entry_path = raw_entry.partition(b"\t")
        if entry_path != raw_path:
            continue
        parts = header.split()
        if len(parts) != 3:
            raise RuntimeError(f"unexpected git index entry for {os.fsdecode(raw_path)!r}")
        mode, object_id, stage = parts
        content = _git_bytes(root, "cat-file", "blob", os.fsdecode(object_id))
        entries.append((b"mode=" + mode + b";stage=" + stage, content))
    return entries


def dirty_digest(root: Path, *, excluded_paths: Iterable[Path] = ()) -> str:
    """Hash staged, unstaged and scoped untracked source bytes.

    The index and worktree are distinct domains so staging a different byte
    sequence cannot collapse into the same receipt. `excluded_paths` removes
    generated proof roots from the untracked scope and prevents a manifest
    from changing its own source identity when it is atomically published.
    """

    digest = hashlib.sha256()
    digest.update(b"quanta-index-dirty-v2\0")
    exclusions = tuple(
        sorted(
            raw for path in excluded_paths if (raw := _repo_relative_bytes(root, path)) is not None
        )
    )

    staged = _git_bytes(
        root,
        "diff",
        "--cached",
        "--name-only",
        "--no-renames",
        "--ignore-submodules=none",
        "-z",
        "HEAD",
        "--",
    )
    for raw_path in sorted(item for item in staged.split(b"\0") if item):
        if _is_excluded(raw_path, exclusions):
            continue
        entries = _index_entries(root, raw_path)
        if not entries:
            _digest_record(digest, b"index", raw_path, b"missing", b"")
        for metadata, content in entries:
            _digest_record(digest, b"index", raw_path, metadata, content)

    unstaged = _git_bytes(
        root,
        "diff",
        "--name-only",
        "--no-renames",
        "--ignore-submodules=none",
        "-z",
        "--",
    )
    for raw_path in sorted(item for item in unstaged.split(b"\0") if item):
        if _is_excluded(raw_path, exclusions):
            continue
        metadata, content = _working_tree_bytes(root, raw_path)
        _digest_record(digest, b"worktree", raw_path, metadata, content)

    untracked = _git_bytes(root, "ls-files", "--others", "--exclude-standard", "-z")
    for raw_path in sorted(item for item in untracked.split(b"\0") if item):
        if _is_excluded(raw_path, exclusions):
            continue
        metadata, content = _working_tree_bytes(root, raw_path)
        _digest_record(digest, b"untracked", raw_path, metadata, content)
    return f"sha256:{digest.hexdigest()}"


def source_snapshot(root: Path, *, excluded_paths: Iterable[Path] = ()) -> dict[str, Any]:
    """Return the exact Git identity used by source-bound proof manifests."""

    head = _git(root, "rev-parse", "--verify", "HEAD")
    branch = _optional_git(root, "symbolic-ref", "--quiet", "--short", "HEAD")
    upstream = _optional_git(
        root,
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        "@{upstream}",
    )
    merge_base = (
        _optional_git(root, "merge-base", "HEAD", upstream) if upstream is not None else None
    )
    return {
        "head": head,
        "dirty_digest": dirty_digest(root, excluded_paths=excluded_paths),
        "branch": branch,
        "upstream": upstream,
        "merge_base": merge_base,
    }


def proof_source_snapshot(
    root: Path,
    *,
    manifest_path: Path,
    proof: dict[str, Any],
    excluded_paths: Iterable[Path] = (),
) -> dict[str, Any]:
    artifact = Path(proof["artifact"])
    artifact_root = artifact.parent
    exclusions = [manifest_path, artifact]
    if artifact_root != Path("."):
        exclusions.append(artifact_root)
    exclusions.extend(excluded_paths)
    return source_snapshot(root, excluded_paths=exclusions)


def _github_repository_from_origin(origin: str) -> str | None:
    patterns = (
        r"^https://github\.com/(?P<path>[^/\s]+/[^/\s]+?)(?:\.git)?/?$",
        r"^git@github[^:]*:(?P<path>[^/\s]+/[^/\s]+?)(?:\.git)?$",
        r"^ssh://git@github[^/]*/(?P<path>[^/\s]+/[^/\s]+?)(?:\.git)?/?$",
    )
    for pattern in patterns:
        match = re.fullmatch(pattern, origin)
        if match is not None:
            return f"github:{match.group('path')}"
    return None


def paired_source_snapshot(
    checkout: Path,
    *,
    repository: str,
    dependency_lock: Path,
) -> dict[str, Any]:
    """Bind an exact external checkout without recording its host-local path."""

    checkout = checkout.resolve()
    top_level = Path(_git(checkout, "rev-parse", "--show-toplevel")).resolve()
    if top_level != checkout:
        raise ValueError(f"paired checkout is not its repository root: {checkout}")
    origin = _git(checkout, "remote", "get-url", "origin")
    actual_repository = _github_repository_from_origin(origin)
    if actual_repository != repository:
        raise ValueError(
            f"paired checkout origin identifies {actual_repository!r}, expected {repository!r}"
        )
    lock_value = dependency_lock.as_posix()
    lock_path, path_error = _payload_repo_file(
        checkout,
        lock_value,
        label="paired dependency lock",
    )
    if path_error is not None:
        raise ValueError(path_error)
    assert lock_path is not None
    if not lock_path.is_file():
        raise ValueError(f"paired dependency lock is missing: {lock_value!r}")
    # Bind the canonical repository identity, not the transport spelling of
    # the remote. The same trusted GitHub repository may be checked out via an
    # HTTPS URL or an SSH host alias; that must not fork release receipts.
    remote_digest = hashlib.sha256()
    remote_digest.update(b"quanta-index-remote-identity-v1\0")
    remote_digest.update(repository.encode("utf-8"))
    return {
        "repository": repository,
        "remote_identity_digest": f"sha256:{remote_digest.hexdigest()}",
        "source": source_snapshot(checkout),
        "dependency_lock": {
            "path": lock_value,
            "sha256": _sha256(lock_path),
        },
    }


def check_manifest(
    payload: Any,
    *,
    manifest_path: Path,
    proof: dict[str, Any],
    schema: dict[str, Any],
    root: Path,
    bind_source: bool,
    allow_non_passed: bool = False,
    paired_checkouts: dict[str, Path] | None = None,
    proof_by_id: dict[str, dict[str, Any]] | None = None,
) -> list[Finding]:
    findings: list[Finding] = []
    validator = jsonschema.Draft202012Validator(schema, format_checker=jsonschema.FormatChecker())
    for error in sorted(validator.iter_errors(payload), key=lambda item: list(item.path)):
        location = ".".join(str(part) for part in error.path) or "root"
        findings.append(Finding(manifest_path, f"schema {location}: {error.message}"))
    if not isinstance(payload, dict) or findings:
        return findings
    if payload["proof_id"] != proof["id"]:
        findings.append(Finding(manifest_path, f"proof_id is not registry id {proof['id']!r}"))
    if payload["family"] != proof["family"]:
        findings.append(
            Finding(manifest_path, f"family is not registry family {proof['family']!r}")
        )
    if payload["status"] != "passed" and not allow_non_passed:
        findings.append(
            Finding(
                manifest_path,
                f"authoritative proof status must be 'passed', got {payload['status']!r}",
            )
        )
    source_pair = payload["source_pair"]
    paired_checkout: Path | None = None
    if proof["source_binding"] == "exact":
        if source_pair is not None:
            findings.append(
                Finding(manifest_path, "source_binding='exact' requires source_pair=null")
            )
    else:
        if not isinstance(source_pair, dict):
            findings.append(
                Finding(
                    manifest_path,
                    "source_binding='exact-pair' requires a source_pair object",
                )
            )
        else:
            repository = proof["paired_repository"]
            dependency_lock = proof["paired_dependency_lock"]
            if source_pair["repository"] != repository:
                findings.append(
                    Finding(manifest_path, "source_pair.repository differs from proof authority")
                )
            if source_pair["dependency_lock"]["path"] != dependency_lock:
                findings.append(
                    Finding(
                        manifest_path,
                        "source_pair.dependency_lock.path differs from proof authority",
                    )
                )
            if bind_source:
                paired_checkout = (paired_checkouts or {}).get(repository)
                if paired_checkout is None:
                    findings.append(
                        Finding(
                            manifest_path,
                            f"source_pair requires --paired-checkout {repository}=PATH",
                        )
                    )
                else:
                    try:
                        live_pair = paired_source_snapshot(
                            paired_checkout,
                            repository=repository,
                            dependency_lock=Path(dependency_lock),
                        )
                    except (OSError, RuntimeError, ValueError) as error:
                        findings.append(Finding(manifest_path, f"cannot bind source_pair: {error}"))
                    else:
                        if source_pair != live_pair:
                            findings.append(
                                Finding(manifest_path, "source_pair is not current paired source")
                            )
    if payload["invocation"]["command"] != proof["command"]:
        findings.append(Finding(manifest_path, "invocation.command differs from proof authority"))
    for field in ("profile", "target", "filter"):
        if payload["invocation"][field] != proof[field]:
            findings.append(
                Finding(manifest_path, f"invocation.{field} differs from proof authority")
            )
    if (
        proof["required_host"] != "any"
        and payload["environment"]["host"]["profile"] != proof["required_host"]
    ):
        findings.append(
            Finding(manifest_path, f"host profile is not required {proof['required_host']!r}")
        )
    if (
        proof["required_host"] == "linux-production-like"
        and payload["environment"]["os"] != "linux"
    ):
        findings.append(
            Finding(manifest_path, "linux-production-like proof requires environment.os='linux'")
        )

    counts = payload["counts"]
    if counts["selected"] != counts["executed"] + counts["ignored"]:
        findings.append(Finding(manifest_path, "selected must equal executed + ignored"))
    if counts["executed"] != counts["passed"] + counts["failed"]:
        findings.append(Finding(manifest_path, "executed must equal passed + failed"))
    if payload["status"] == "passed":
        if counts["selected"] == 0 or counts["executed"] == 0 or counts["passed"] == 0:
            findings.append(
                Finding(manifest_path, "passed proof requires selected, executed and passed > 0")
            )
        if counts["failed"] != 0:
            findings.append(Finding(manifest_path, "passed proof cannot contain failed tests"))
        if counts["ignored"] != 0:
            findings.append(
                Finding(manifest_path, "mandatory passed proof cannot contain ignored tests")
            )
    if _parse_time(payload["ended_at"]) < _parse_time(payload["started_at"]):
        findings.append(Finding(manifest_path, "ended_at precedes started_at"))

    daemon_binary = payload["daemon_binary"]
    if proof["binary_binding"] == "none":
        if daemon_binary is not None:
            findings.append(
                Finding(manifest_path, "binary_binding='none' requires daemon_binary=null")
            )
    elif not isinstance(daemon_binary, dict):
        findings.append(
            Finding(
                manifest_path,
                "binary_binding='release-daemon' requires a daemon_binary object",
            )
        )
    else:
        daemon_path, path_error = _payload_repo_file(
            root,
            daemon_binary["path"],
            label="daemon binary",
        )
        if path_error is not None:
            findings.append(Finding(manifest_path, path_error))
        elif daemon_path is None or not daemon_path.is_file():
            findings.append(Finding(manifest_path, f"daemon binary is missing: {daemon_path}"))
        elif _sha256(daemon_path) != daemon_binary["sha256"]:
            findings.append(Finding(manifest_path, "daemon binary digest mismatch"))
    for artifact in payload["artifacts"]:
        artifact_path, path_error = _payload_repo_file(
            root,
            artifact["path"],
            label="proof artifact",
        )
        if path_error is not None:
            findings.append(Finding(manifest_path, path_error))
        elif artifact_path is None or not artifact_path.is_file():
            findings.append(Finding(manifest_path, f"proof artifact is missing: {artifact_path}"))
        elif _sha256(artifact_path) != artifact["sha256"]:
            findings.append(
                Finding(manifest_path, f"proof artifact digest mismatch: {artifact_path}")
            )

    dependencies = payload["dependency_receipts"]
    if {item["proof_id"] for item in dependencies} != set(proof["dependencies"]):
        findings.append(
            Finding(manifest_path, "dependency receipt IDs differ from proof authority")
        )
    for dependency in dependencies:
        if proof_by_id is not None:
            dependency_authority = proof_by_id.get(dependency["proof_id"])
            if dependency_authority is None:
                findings.append(
                    Finding(
                        manifest_path,
                        f"dependency receipt names unknown proof {dependency['proof_id']!r}",
                    )
                )
                continue
            if dependency["path"] != dependency_authority["artifact"]:
                findings.append(
                    Finding(
                        manifest_path,
                        f"dependency receipt path differs from registered artifact: {dependency['proof_id']!r}",
                    )
                )
                continue
        dependency_path, path_error = _payload_repo_file(
            root,
            dependency["path"],
            label="dependency receipt",
        )
        if path_error is not None:
            findings.append(Finding(manifest_path, path_error))
        elif dependency_path is None or not dependency_path.is_file():
            findings.append(
                Finding(manifest_path, f"dependency receipt is missing: {dependency_path}")
            )
        elif _sha256(dependency_path) != dependency["sha256"]:
            findings.append(
                Finding(manifest_path, f"dependency receipt digest mismatch: {dependency_path}")
            )

    if bind_source:
        current_source = proof_source_snapshot(
            root,
            manifest_path=manifest_path,
            proof=proof,
            excluded_paths=(() if paired_checkout is None else (paired_checkout,)),
        )
        labels = {
            "head": "current HEAD",
            "dirty_digest": "current working tree",
            "branch": "current branch",
            "upstream": "current upstream",
            "merge_base": "current upstream merge-base",
        }
        for field, label in labels.items():
            if payload["source"][field] != current_source[field]:
                findings.append(Finding(manifest_path, f"source.{field} is not {label}"))
    return findings


def dependency_closure(proof_by_id: dict[str, dict[str, Any]], target_id: str) -> list[str]:
    """Return a target's transitive dependencies in dependency-first order."""

    if target_id not in proof_by_id:
        raise ValueError(f"unknown proof id {target_id!r}")
    ordered: list[str] = []
    visited: set[str] = set()
    active: set[str] = set()

    def visit(proof_id: str) -> None:
        if proof_id in active:
            raise ValueError(f"proof dependency cycle reaches {proof_id!r}")
        active.add(proof_id)
        for dependency in proof_by_id[proof_id]["dependencies"]:
            if dependency not in proof_by_id:
                raise ValueError(f"proof {proof_id!r} depends on unknown proof {dependency!r}")
            if dependency not in visited:
                visit(dependency)
                visited.add(dependency)
                ordered.append(dependency)
        active.remove(proof_id)

    visit(target_id)
    return ordered


def check_aggregate(
    payload_by_id: dict[str, dict[str, Any]],
    *,
    proof_by_id: dict[str, dict[str, Any]],
    path: Path,
) -> list[Finding]:
    findings: list[Finding] = []
    exact_sources = {
        json.dumps(payload["source"], sort_keys=True, separators=(",", ":"))
        for proof_id, payload in payload_by_id.items()
        if proof_by_id[proof_id]["source_binding"] == "exact"
    }
    if len(exact_sources) > 1:
        findings.append(
            Finding(path, "aggregate exact-source manifests do not share one source identity")
        )
    daemon_identities = {
        (payload["daemon_binary"]["path"], payload["daemon_binary"]["sha256"])
        for proof_id, payload in payload_by_id.items()
        if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
        and isinstance(payload["daemon_binary"], dict)
    }
    if len(daemon_identities) > 1:
        findings.append(
            Finding(
                path,
                "aggregate release-daemon proofs do not share one daemon path and digest",
            )
        )
    return findings


def _parse_paired_checkouts(values: list[str]) -> tuple[dict[str, Path], list[str]]:
    checkouts: dict[str, Path] = {}
    errors: list[str] = []
    for value in values:
        repository, separator, raw_path = value.partition("=")
        if not separator or not repository or not raw_path:
            errors.append(f"invalid --paired-checkout {value!r}; expected REPOSITORY=PATH")
            continue
        if repository in checkouts:
            errors.append(f"duplicate --paired-checkout for {repository!r}")
            continue
        checkouts[repository] = Path(raw_path)
    return checkouts, errors


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--registry", type=Path, default=None)
    parser.add_argument("--schema", type=Path, default=None)
    parser.add_argument("--manifest", action="append", type=Path, default=[])
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--require-all", action="store_true")
    selection.add_argument("--dependencies-of", metavar="PROOF_ID")
    parser.add_argument(
        "--paired-checkout",
        action="append",
        default=[],
        metavar="REPOSITORY=PATH",
    )
    parser.add_argument("--bind-source", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    root = args.root.resolve()
    registry_path = (args.registry or root / "tools/ci/proof-authority.toml").resolve()
    schema_path = (args.schema or root / "tools/ci/proof-manifest.schema.json").resolve()
    try:
        registry = _read_toml(registry_path)
        schema = _read_json(schema_path)
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2

    findings = check_registry(registry, root=root, path=registry_path)
    proof_by_id = {
        proof["id"]: proof
        for proof in registry.get("proofs", [])
        if isinstance(proof, dict) and isinstance(proof.get("id"), str)
    }
    paired_checkouts, paired_errors = _parse_paired_checkouts(args.paired_checkout)
    findings.extend(Finding(registry_path, message) for message in paired_errors)
    paired_repositories = {
        proof["paired_repository"]
        for proof in proof_by_id.values()
        if proof.get("source_binding") == "exact-pair"
        and isinstance(proof.get("paired_repository"), str)
    }
    for repository in paired_checkouts.keys() - paired_repositories:
        findings.append(
            Finding(registry_path, f"--paired-checkout names unknown repository {repository!r}")
        )
    manifest_paths = [path.resolve() for path in args.manifest]
    expected_proof_by_path: dict[Path, str] = {}

    def require_registered_manifest(proof_id: str) -> None:
        proof = proof_by_id[proof_id]
        manifest_path, path_error = _payload_repo_file(
            root,
            proof["artifact"],
            label=f"proof {proof_id!r} manifest artifact",
        )
        if path_error is not None:
            findings.append(Finding(registry_path, path_error))
            return
        assert manifest_path is not None
        manifest_paths.append(manifest_path)
        expected_proof_by_path[manifest_path] = proof_id

    if args.require_all:
        if not args.bind_source:
            findings.append(Finding(registry_path, "--require-all requires --bind-source"))
        for proof_id in proof_by_id:
            require_registered_manifest(proof_id)
    if args.dependencies_of is not None:
        if not args.bind_source:
            findings.append(Finding(registry_path, "--dependencies-of requires --bind-source"))
        try:
            dependency_ids = dependency_closure(proof_by_id, args.dependencies_of)
        except ValueError as error:
            findings.append(Finding(registry_path, str(error)))
        else:
            for proof_id in dependency_ids:
                require_registered_manifest(proof_id)
    seen_paths: set[Path] = set()
    seen_proofs: set[str] = set()
    payload_by_id: dict[str, dict[str, Any]] = {}
    for manifest_path in manifest_paths:
        if manifest_path in seen_paths:
            continue
        seen_paths.add(manifest_path)
        if not manifest_path.is_file():
            findings.append(Finding(manifest_path, "required proof manifest is missing"))
            continue
        try:
            payload = _read_json(manifest_path)
        except (OSError, json.JSONDecodeError) as error:
            findings.append(Finding(manifest_path, f"unreadable proof manifest: {error}"))
            continue
        proof_id = payload.get("proof_id") if isinstance(payload, dict) else None
        expected_proof_id = expected_proof_by_path.get(manifest_path)
        if expected_proof_id is not None and proof_id != expected_proof_id:
            findings.append(
                Finding(
                    manifest_path,
                    f"registered artifact requires proof_id {expected_proof_id!r}, got {proof_id!r}",
                )
            )
            continue
        proof = proof_by_id.get(proof_id)
        if proof is None:
            findings.append(Finding(manifest_path, f"proof_id {proof_id!r} is not registered"))
            continue
        if proof_id in seen_proofs:
            findings.append(Finding(manifest_path, f"duplicate manifest for proof_id {proof_id!r}"))
            continue
        seen_proofs.add(proof_id)
        manifest_findings = check_manifest(
            payload,
            manifest_path=manifest_path,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=args.bind_source,
            paired_checkouts=paired_checkouts,
            proof_by_id=proof_by_id,
        )
        findings.extend(manifest_findings)
        if not manifest_findings:
            payload_by_id[proof_id] = payload

    if args.require_all or args.dependencies_of is not None:
        findings.extend(
            check_aggregate(
                payload_by_id,
                proof_by_id=proof_by_id,
                path=registry_path,
            )
        )

    if findings:
        for finding in findings:
            print(f"REFUSED {finding.render()}", file=sys.stderr)
        print(f"FAIL: {len(findings)} proof-authority finding(s)", file=sys.stderr)
        return 1
    print(f"OK: {len(proof_by_id)} registered proof(s); {len(seen_proofs)} manifest(s) validated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
