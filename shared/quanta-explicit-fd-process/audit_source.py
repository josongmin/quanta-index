#!/usr/bin/env python3
"""Verify the reviewed process/unsafe source inventory without regenerating it."""

from __future__ import annotations

import hashlib
import json
import os
import stat
import sys
from pathlib import Path, PurePosixPath

PACKAGE_ROOT = Path(__file__).resolve().parent
BASELINE_NAME = "audited-source.json"
RELOCATION_SOURCE_COUNT = 27
RELOCATION_INVENTORY_SHA256 = "776ad3d561411322c937ac2d85bb40fe8cdcd23661fb75b466355eb2348e6f56"
ORIGINAL_COHORT_SHA256 = "48e576b81fb439b8cacafd2654437766835cb4ca5acfdade0d98c7198656d565"
ORIGINAL_TESTS_SHA256 = "90d85ad101826f42d993837186ef2c08c95cc39d2e19a6e8da4937a88971620b"
FORMER_SEMANTICA_AUTHORITIES = {
    "unrelated_cloexec_descriptor_is_absent_from_child_v1": "ccf50511ee9bea750235e9562c2e51b26e1b7a2a4b275623ba6e99cd13e1a522",
    "test_helper_request_v1": "0b1aa6c46da455cc54731c2f63d96d918709863c555b0ab08105aefb5ec85501",
    "pipe_helper_request_v1": "7b62d894f23db07980e922371b669fbd8b9e9ac8e932be9d2cc53e2c174edbc2",
}
MAX_BASELINE_BYTES = 64 * 1024
MAX_AUDITED_FILE_BYTES = 2 * 1024 * 1024


def _unique_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _read_regular(path: Path, byte_limit: int) -> bytes:
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
    descriptor = os.open(path, flags)
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_size > byte_limit:
            raise ValueError(f"nonregular or oversized audited file: {path}")
        with os.fdopen(os.dup(descriptor), "rb") as stream:
            data = stream.read(byte_limit + 1)
        if len(data) > byte_limit:
            raise ValueError(f"oversized audited file: {path}")
        return data
    finally:
        os.close(descriptor)


def _source_paths(root: Path) -> set[str]:
    source_root = root / "src"
    if not stat.S_ISDIR(source_root.lstat().st_mode):
        raise ValueError("src must be a real directory")
    observed: set[str] = set()

    def fail_on_walk_error(error: OSError) -> None:
        raise error

    for current, directory_names, file_names in os.walk(
        source_root, followlinks=False, onerror=fail_on_walk_error
    ):
        for name in directory_names:
            path = Path(current) / name
            if not stat.S_ISDIR(path.lstat().st_mode):
                raise ValueError(f"nonregular source directory: {path}")
        for name in file_names:
            path = Path(current) / name
            if not stat.S_ISREG(path.lstat().st_mode):
                raise ValueError(f"nonregular source file: {path}")
            if path.suffix == ".rs":
                observed.add(path.relative_to(root).as_posix())
    return observed


def _is_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def audit_package(root: Path) -> list[str]:
    """Return explicit failures; never update or infer expected hashes."""
    try:
        raw = _read_regular(root / BASELINE_NAME, MAX_BASELINE_BYTES)
        baseline = json.loads(raw, object_pairs_hook=_unique_keys)
        if not isinstance(baseline, dict) or set(baseline) != {
            "schema_version",
            "source_count",
            "original_cohort_sha256",
            "original_tests_sha256",
            "former_semantica_authorities",
            "manifest_sha256",
            "files",
            "reviewed_changes",
        }:
            return ["invalid audit baseline schema"]
        if baseline["schema_version"] != 2 or baseline["source_count"] != RELOCATION_SOURCE_COUNT:
            return ["invalid audit baseline version or source count"]
        if (
            baseline["original_cohort_sha256"] != ORIGINAL_COHORT_SHA256
            or baseline["original_tests_sha256"] != ORIGINAL_TESTS_SHA256
            or baseline["former_semantica_authorities"] != FORMER_SEMANTICA_AUTHORITIES
        ):
            return ["audit provenance differs from frozen Semantica source guard"]
        relocation = baseline["files"]
        reviewed_changes = baseline["reviewed_changes"]
        if (
            not isinstance(relocation, dict)
            or len(relocation) != RELOCATION_SOURCE_COUNT
            or not isinstance(reviewed_changes, dict)
        ):
            return ["invalid audited source inventory"]
        relocation_bytes = json.dumps(relocation, sort_keys=True, separators=(",", ":")).encode(
            "utf-8"
        )
        if hashlib.sha256(relocation_bytes).hexdigest() != RELOCATION_INVENTORY_SHA256:
            return ["relocation source inventory changed"]
        files = relocation | reviewed_changes
        for relative, expected in files.items():
            if not isinstance(relative, str) or not _is_sha256(expected):
                return ["invalid audited source path or digest"]
            path = PurePosixPath(relative)
            if (
                path.is_absolute()
                or path.parts[:1] != ("src",)
                or any(part in (".", "..") for part in path.parts)
                or path.as_posix() != relative
                or path.suffix != ".rs"
            ):
                return [f"invalid audited source path: {relative}"]
        if not _is_sha256(baseline["manifest_sha256"]):
            return ["invalid manifest digest"]
        observed = _source_paths(root)
        failures = [
            f"missing audited Rust source: {path}" for path in sorted(files.keys() - observed)
        ]
        failures.extend(
            f"unexpected Rust source: {path}" for path in sorted(observed - files.keys())
        )
        for relative in sorted(files.keys() & observed):
            actual = hashlib.sha256(
                _read_regular(root / relative, MAX_AUDITED_FILE_BYTES)
            ).hexdigest()
            if actual != files[relative]:
                failures.append(f"changed audited Rust source: {relative}")
        actual_manifest = hashlib.sha256(
            _read_regular(root / "Cargo.toml", MAX_AUDITED_FILE_BYTES)
        ).hexdigest()
        if actual_manifest != baseline["manifest_sha256"]:
            failures.append("changed audited Cargo.toml")
        return failures
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as error:
        return [f"invalid or missing audited package input: {error}"]


def main() -> int:
    failures = audit_package(PACKAGE_ROOT)
    if failures:
        for failure in failures:
            print(f"FAILED: {failure}", file=sys.stderr)
        return 1
    print("VERIFIED: audited Rust source inventory and Cargo.toml")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
