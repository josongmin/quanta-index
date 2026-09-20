#!/usr/bin/env python3
"""Fail closed when an ignored Rust test lacks an owned, expiring exception."""

from __future__ import annotations

import argparse
import datetime as dt
import re
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
DEFAULT_POLICY = ROOT / "tools" / "ci" / "ignored-test-policy.toml"
IGNORE = re.compile(r'ignore\s*=\s*"([^"]+)"')
FUNCTION = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(")


def _ignored_tests(root: Path) -> set[tuple[str, str, str]]:
    found: set[tuple[str, str, str]] = set()
    for source in root.glob("crates/**/*.rs"):
        lines = source.read_text(encoding="utf-8").splitlines()
        for index, line in enumerate(lines):
            match = IGNORE.search(line)
            if match is None:
                continue
            function = next(
                (
                    candidate.group(1)
                    for candidate_line in lines[index + 1 : index + 10]
                    if (candidate := FUNCTION.search(candidate_line))
                ),
                None,
            )
            if function is None:
                raise ValueError(f"cannot identify ignored test function: {source}:{index + 1}")
            found.add((source.relative_to(root).as_posix(), function, match.group(1)))
    return found


def audit(root: Path = ROOT, policy_path: Path = DEFAULT_POLICY) -> list[str]:
    try:
        policy = tomllib.loads(policy_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        return [f"{policy_path}: cannot parse ignored-test policy: {error}"]
    rows = policy.get("exceptions")
    if not isinstance(rows, list):
        return [f"{policy_path}: exceptions must be an array"]
    declared: set[tuple[str, str, str]] = set()
    errors: list[str] = []
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            errors.append(f"{policy_path}: exceptions[{index}] must be a table")
            continue
        values = tuple(row.get(key) for key in ("path", "test", "reason"))
        if not all(isinstance(value, str) and value for value in values):
            errors.append(f"{policy_path}: exceptions[{index}] requires path, test, reason")
            continue
        owner = row.get("owner")
        cadence = row.get("cadence")
        review_by = row.get("review_by")
        if not isinstance(owner, str) or not owner or not isinstance(cadence, str) or not cadence:
            errors.append(f"{policy_path}: exceptions[{index}] requires owner and cadence")
        try:
            review_date = dt.date.fromisoformat(str(review_by))
        except ValueError:
            errors.append(f"{policy_path}: exceptions[{index}].review_by must be ISO date")
        else:
            if review_date < dt.date.today():
                errors.append(
                    f"{policy_path}: exceptions[{index}] review_by is expired: {review_by}"
                )
        entry = values  # type: ignore[assignment]
        if entry in declared:
            errors.append(f"{policy_path}: duplicate exception: {entry[0]}::{entry[1]}")
        declared.add(entry)
    actual = _ignored_tests(root)
    for entry in sorted(actual - declared):
        errors.append(f"unowned ignored test: {entry[0]}::{entry[1]}")
    for entry in sorted(declared - actual):
        errors.append(f"stale ignored-test exception: {entry[0]}::{entry[1]}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    args = parser.parse_args()
    errors = audit(args.root.resolve(), args.policy.resolve())
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"ignored-test policy: OK ({args.policy})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
