#!/usr/bin/env python3
"""Capture and verify an exact, profile-scoped Git source closure."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path


SCHEMA_VERSION = 1
PROFILES = {
    "retrieval": {
        "cargo_packages": (
            "quanta-index-retrieval-bench",
            "quanta-index-searchd-runtime",
        ),
        "paths": (
            ".cargo/config.toml",
            "Cargo.lock",
            "Cargo.toml",
            "Justfile",
            "docs/plans/sep-23-retrieval-bench",
            "scripts/cargow",
            "scripts/quanta-index-env.sh",
            "rust-toolchain.toml",
            "tools/benchmark/retrieval",
            "tools/ci/lint/check-rust-derive-allowlist.py",
            "tools/ci/lint/check-test-authority.py",
            "tools/ci/source_closure.py",
            "tools/ci/timing/rust_profile_history.py",
            "tools/ci/tests/test_retrieval_benchmark.py",
            "tools/ci/tests/test_retrieval_contract_proof.py",
            "tools/ci/tests/test_retrieval_sdk_proof.py",
            "tools/ci/tests/test_write_verification_receipt.py",
            "tools/ci/verification-receipt.schema.json",
            "tools/ci/write-verification-receipt.py",
        ),
    },
}


class ClosureError(RuntimeError):
    """A source closure cannot be captured or verified."""


def _git(repo: Path, *args: str) -> str:
    try:
        return subprocess.check_output(
            ["git", *args], cwd=repo, text=True, stderr=subprocess.STDOUT
        ).strip()
    except subprocess.CalledProcessError as error:
        raise ClosureError(error.output.strip() or f"git {' '.join(args)} failed") from error


def _repo_root(start: Path | None = None) -> Path:
    return Path(_git((start or Path.cwd()).resolve(), "rev-parse", "--show-toplevel")).resolve()


def _canonical(payload: object) -> bytes:
    return json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def _digest(payload: object) -> str:
    return hashlib.sha256(_canonical(payload)).hexdigest()


def _metadata(repo: Path) -> dict:
    wrapper = repo / "scripts" / "cargow"
    try:
        raw = subprocess.check_output(
            [str(wrapper), "--lane", "metadata-lane", "metadata", "--format-version", "1", "--locked"],
            cwd=repo,
        )
    except subprocess.CalledProcessError as error:
        raise ClosureError("cargo metadata failed while resolving source closure") from error
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as error:
        raise ClosureError("cargo metadata returned invalid JSON") from error
    if not isinstance(payload, dict):
        raise ClosureError("cargo metadata must return an object")
    return payload


def _cargo_roots(repo: Path, package_names: tuple[str, ...]) -> set[str]:
    if not package_names:
        return set()
    metadata = _metadata(repo)
    packages = metadata.get("packages")
    nodes = (metadata.get("resolve") or {}).get("nodes")
    if not isinstance(packages, list) or not isinstance(nodes, list):
        raise ClosureError("cargo metadata omitted packages or resolve nodes")
    by_id = {package.get("id"): package for package in packages if isinstance(package, dict)}
    node_by_id = {node.get("id"): node for node in nodes if isinstance(node, dict)}
    wanted = {
        package["id"]
        for package in packages
        if isinstance(package, dict)
        and package.get("name") in package_names
        and package.get("source") is None
    }
    found_names = {by_id[package_id]["name"] for package_id in wanted}
    missing = sorted(set(package_names) - found_names)
    if missing:
        raise ClosureError(f"cargo packages missing from workspace: {', '.join(missing)}")
    pending = list(wanted)
    closure: set[str] = set()
    while pending:
        package_id = pending.pop()
        if package_id in closure:
            continue
        package = by_id.get(package_id)
        node = node_by_id.get(package_id)
        if not package or package.get("source") is not None or not node:
            continue
        closure.add(package_id)
        dependencies = node.get("dependencies")
        if not isinstance(dependencies, list):
            raise ClosureError(f"cargo resolve node has invalid dependencies: {package_id}")
        pending.extend(dependencies)

    roots: set[str] = set()
    for package_id in closure:
        manifest = Path(by_id[package_id]["manifest_path"]).resolve()
        try:
            relative = manifest.parent.relative_to(repo).as_posix()
        except ValueError as error:
            raise ClosureError(f"local cargo package escaped repository: {manifest}") from error
        roots.add(relative or ".")
    return roots


def resolve_roots(repo: Path, profile: str, extra_paths: tuple[str, ...] = ()) -> list[str]:
    if profile not in PROFILES:
        raise ClosureError(f"unknown source closure profile: {profile}")
    config = PROFILES[profile]
    roots = set(config["paths"])
    roots.update(_cargo_roots(repo, config["cargo_packages"]))
    roots.update(extra_paths)
    normalized: set[str] = set()
    for value in roots:
        candidate = (repo / value).resolve()
        try:
            relative = candidate.relative_to(repo).as_posix()
        except ValueError as error:
            raise ClosureError(f"source root escaped repository: {value}") from error
        if not candidate.exists() and not candidate.is_symlink():
            raise ClosureError(f"source root does not exist: {relative}")
        normalized.add(relative)
    return sorted(normalized)


def _files(repo: Path, roots: list[str]) -> list[str]:
    output = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *roots],
        cwd=repo,
    )
    paths = sorted({part.decode() for part in output.split(b"\0") if part})
    if not paths:
        raise ClosureError("source closure contains no files")
    for path in paths:
        candidate = repo / path
        if candidate.is_symlink():
            raise ClosureError(f"source closure refuses symlink: {path}")
        if not candidate.is_file():
            raise ClosureError(f"source closure path is not a regular file: {path}")
    return paths


def _assert_clean(repo: Path, roots: list[str]) -> None:
    status = _git(repo, "status", "--porcelain=v1", "--untracked-files=all", "--", *roots)
    if status:
        sample = ", ".join(status.splitlines()[:8])
        raise ClosureError(f"refusing dirty relevant source: {sample}")


def build_manifest(repo: Path, profile: str) -> dict:
    roots = resolve_roots(repo, profile)
    _assert_clean(repo, roots)
    revision = _git(repo, "rev-parse", "HEAD")
    entries = [
        {"path": path, "sha256": hashlib.sha256((repo / path).read_bytes()).hexdigest()}
        for path in _files(repo, roots)
    ]
    core = {
        "schema_version": SCHEMA_VERSION,
        "profile": profile,
        "revision": revision,
        "roots": roots,
        "files": entries,
    }
    return {**core, "digest": _digest(core)}


def validate_manifest_shape(payload: object) -> dict:
    keys = {"schema_version", "profile", "revision", "roots", "files", "digest"}
    if not isinstance(payload, dict) or set(payload) != keys:
        raise ClosureError(f"source closure must hold exactly {sorted(keys)}")
    if payload["schema_version"] != SCHEMA_VERSION:
        raise ClosureError(f"source closure schema_version must be {SCHEMA_VERSION}")
    if not isinstance(payload["profile"], str) or payload["profile"] not in PROFILES:
        raise ClosureError("source closure has unknown profile")
    if not isinstance(payload["revision"], str) or len(payload["revision"]) != 40:
        raise ClosureError("source closure revision must be a full Git SHA")
    roots = payload["roots"]
    if not isinstance(roots, list) or not roots or roots != sorted(set(roots)):
        raise ClosureError("source closure roots must be a nonempty sorted unique list")
    files = payload["files"]
    if not isinstance(files, list) or not files:
        raise ClosureError("source closure files must be a nonempty list")
    paths: list[str] = []
    for index, entry in enumerate(files):
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256"}:
            raise ClosureError(f"source closure files[{index}] has invalid shape")
        if not isinstance(entry["path"], str) or not entry["path"]:
            raise ClosureError(f"source closure files[{index}].path is invalid")
        if (
            not isinstance(entry["sha256"], str)
            or len(entry["sha256"]) != 64
            or any(ch not in "0123456789abcdef" for ch in entry["sha256"])
        ):
            raise ClosureError(f"source closure files[{index}].sha256 is invalid")
        paths.append(entry["path"])
    if paths != sorted(set(paths)):
        raise ClosureError("source closure files must be sorted and unique")
    core = {key: payload[key] for key in ("schema_version", "profile", "revision", "roots", "files")}
    if payload["digest"] != _digest(core):
        raise ClosureError("source closure digest mismatch")
    return payload


def verify_manifest(repo: Path, payload: object) -> dict:
    manifest = validate_manifest_shape(payload)
    current_roots = resolve_roots(repo, manifest["profile"])
    if current_roots != manifest["roots"]:
        raise ClosureError("source closure roots changed")
    _assert_clean(repo, current_roots)
    if _git(repo, "rev-parse", "HEAD") != manifest["revision"]:
        raise ClosureError("source closure revision changed")
    current_paths = _files(repo, current_roots)
    expected_paths = [entry["path"] for entry in manifest["files"]]
    if current_paths != expected_paths:
        raise ClosureError("source closure file set changed")
    for entry in manifest["files"]:
        actual = hashlib.sha256((repo / entry["path"]).read_bytes()).hexdigest()
        if actual != entry["sha256"]:
            raise ClosureError(f"source closure file digest changed: {entry['path']}")
    return manifest


def load_and_verify(path: Path, repo: Path | None = None) -> dict:
    try:
        payload = json.loads(path.read_bytes())
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ClosureError(f"cannot read source closure {path}: {error}") from error
    return verify_manifest(repo or _repo_root(), payload)


def _write_exclusive(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = json.dumps(payload, sort_keys=True, indent=2) + "\n"
    try:
        with path.open("x", encoding="utf-8") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
    except FileExistsError as error:
        raise ClosureError(f"refusing existing source closure: {path}") from error


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    for name in ("check", "capture"):
        child = subparsers.add_parser(name)
        child.add_argument("--profile", required=True, choices=sorted(PROFILES))
        if name == "capture":
            child.add_argument("--out", required=True, type=Path)
    verify = subparsers.add_parser("verify")
    verify.add_argument("--manifest", required=True, type=Path)
    args = parser.parse_args()
    try:
        repo = _repo_root()
        if args.command == "verify":
            manifest = load_and_verify(args.manifest, repo)
        else:
            manifest = build_manifest(repo, args.profile)
            if args.command == "capture":
                _write_exclusive(args.out, manifest)
    except ClosureError as error:
        raise SystemExit(str(error)) from error
    print(
        f"source closure {args.command} ok: {manifest['profile']} "
        f"{len(manifest['files'])} files {manifest['digest']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
