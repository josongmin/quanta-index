#!/usr/bin/env python3
"""Collect the exact pytest identities required by the retrieval contract rail."""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import sys
from pathlib import Path

import pytest

try:
    from tools.ci.nextest_events import NextestEvidenceError, parse_nextest_inventory
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.ci.nextest_events import NextestEvidenceError, parse_nextest_inventory

PYTHON_SELECTOR = "tools/ci/tests/test_retrieval_benchmark.py"
DEFAULT_AUTHORITY = (
    Path(__file__).resolve().parents[3] / "benchmarks/retrieval/proof-required-tests.json"
)


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate proof inventory JSON key: {key}")
        result[key] = value
    return result


def _load_json(path: Path) -> object:
    try:
        with path.open("r", encoding="utf-8") as stream:
            return json.load(stream, object_pairs_hook=_unique_object)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        raise ValueError(f"invalid proof inventory {path}: {error}") from error


def _identities(value: object, label: str) -> list[str]:
    if (
        not isinstance(value, list)
        or not value
        or any(not isinstance(item, str) or not item for item in value)
        or value != sorted(set(value))
    ):
        raise ValueError(f"{label} identities must be sorted, unique, and nonempty")
    return value


def verify_inventory_authority(
    inventory: Path, role: str, authority_path: Path = DEFAULT_AUTHORITY
) -> None:
    """Reject collections that differ from the source-controlled required tests."""
    if role not in {"python", "rust", "sdk"}:
        raise ValueError(f"unknown proof inventory role: {role}")
    authority = _load_json(authority_path)
    if (
        not isinstance(authority, dict)
        or set(authority) != {"schema_version", "python", "rust", "sdk"}
        or type(authority["schema_version"]) is not int
        or authority["schema_version"] != 1
    ):
        raise ValueError("invalid source-controlled proof inventory authority")
    required = _identities(authority[role], f"{role} required")
    if role == "python":
        payload = _load_json(inventory)
        if (
            not isinstance(payload, dict)
            or set(payload) != {"schema_version", "kind", "selector", "tests"}
            or type(payload["schema_version"]) is not int
            or payload["schema_version"] != 1
            or payload["kind"] != "pytest"
            or payload["selector"] != PYTHON_SELECTOR
        ):
            raise ValueError("invalid pytest collection inventory")
        actual = _identities(payload["tests"], "pytest collection")
    else:
        try:
            actual = sorted(parse_nextest_inventory(inventory))
        except NextestEvidenceError as error:
            raise ValueError(f"invalid {role} nextest inventory: {error}") from error
    if actual != required:
        raise ValueError(f"{role} collection differs from source-controlled required tests")


def junit_identity(nodeid: str) -> str:
    path, separator, test = nodeid.partition("::")
    path = path.replace("\\", "/")
    if not separator or not path.endswith(".py") or not test:
        raise ValueError(f"invalid collected pytest nodeid: {nodeid}")
    return f"{path[:-3].replace('/', '.')}.{test.replace('::', '.')}"


def collect_pytest() -> dict[str, object]:
    root = Path(__file__).resolve().parents[3]
    if str(root) not in sys.path:
        sys.path.insert(0, str(root))

    class Collector:
        items: list[str] = []

        def pytest_collection_finish(self, session: pytest.Session) -> None:
            self.items = [item.nodeid for item in session.items]

    plugin = Collector()
    captured = io.StringIO()
    original_cwd = Path.cwd()
    try:
        os.chdir(root)
        with contextlib.redirect_stdout(captured), contextlib.redirect_stderr(captured):
            outcome = pytest.main([PYTHON_SELECTOR, "--collect-only", "-q"], plugins=[plugin])
    finally:
        os.chdir(original_cwd)
    if outcome != pytest.ExitCode.OK:
        raise SystemExit(f"pytest collection failed ({outcome}): {captured.getvalue()}")
    tests = sorted(junit_identity(nodeid) for nodeid in plugin.items)
    if not tests or len(tests) != len(set(tests)):
        raise SystemExit("pytest collection is empty or has duplicate JUnit identities")
    return {
        "schema_version": 1,
        "kind": "pytest",
        "selector": PYTHON_SELECTOR,
        "tests": tests,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--out", type=Path)
    target.add_argument("--verify", type=Path)
    parser.add_argument("--role", choices=("python", "rust", "sdk"))
    args = parser.parse_args()
    if args.verify is not None:
        if args.role is None:
            parser.error("--verify requires --role")
        try:
            verify_inventory_authority(args.verify, args.role)
        except ValueError as error:
            raise SystemExit(str(error)) from error
        return 0
    if args.role is not None:
        parser.error("--role applies only to --verify")
    payload = collect_pytest()
    authority = _load_json(DEFAULT_AUTHORITY)
    if not isinstance(authority, dict) or payload["tests"] != _identities(
        authority.get("python"), "python required"
    ):
        raise SystemExit("pytest collection differs from source-controlled required tests")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("x", encoding="utf-8") as stream:
        stream.write(json.dumps(payload, sort_keys=True, indent=2) + "\n")
        stream.flush()
        os.fsync(stream.fileno())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
