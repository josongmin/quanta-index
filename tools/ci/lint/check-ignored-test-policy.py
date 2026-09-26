#!/usr/bin/env python3
"""Fail closed when an ignored Rust test lacks an owned, expiring exception."""

from __future__ import annotations

import argparse
import datetime as dt
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
DEFAULT_POLICY = ROOT / "tools" / "ci" / "ignored-test-policy.toml"
LINT_DIR = str(Path(__file__).resolve().parent)
if LINT_DIR not in sys.path:
    sys.path.insert(0, LINT_DIR)
from rust_attribute_policy import (  # noqa: E402
    attribute_metas,
    macro_attribute_metas,
    rust_source_files,
    string_value,
)


def _ignored_tests(root: Path) -> set[tuple[str, str, str]]:
    from tree_sitter_language_pack import get_parser

    found: set[tuple[str, str, str]] = set()
    parser = get_parser("rust")
    for source in rust_source_files((root / "crates", root / "benchmarks")):
        tree = parser.parse(source.read_bytes())
        targets: dict[str, int] = {}

        def visit(node: object, source: Path = source, targets: dict[str, int] = targets) -> None:
            if node.type in {"macro_invocation", "macro_definition"}:
                for line, metas in macro_attribute_metas(node):
                    if any(name == "ignore" for name, _, _ in metas):
                        raise ValueError(
                            f"cannot identify ignored test in opaque Rust macro: {source}:{line}"
                        )
                return
            children = node.children
            for index, child in enumerate(children):
                if child.type == "attribute_item":
                    ignores = [
                        (arguments, conditional)
                        for name, arguments, conditional in attribute_metas(child)
                        if name == "ignore"
                    ]
                    if ignores:
                        target = next(
                            (
                                candidate
                                for candidate in children[index + 1 :]
                                if candidate.type
                                not in {"attribute_item", "line_comment", "block_comment"}
                            ),
                            None,
                        )
                        name = target.child_by_field_name("name") if target is not None else None
                        if target is None or target.type != "function_item" or name is None:
                            raise ValueError(
                                f"cannot identify ignored test function: {source}:{child.start_point.row + 1}"
                            )
                        test_name = name.text.decode("utf-8").removeprefix("r#")
                        if test_name in targets and targets[test_name] != target.start_byte:
                            raise ValueError(
                                f"ambiguous ignored-test function identity: {source}::{test_name}"
                            )
                        targets[test_name] = target.start_byte
                        for arguments, conditional in ignores:
                            if not arguments:
                                reason = "<conditional ignore>" if conditional else ""
                            elif len(arguments) == 2 and arguments[0].type == "=":
                                reason = string_value(arguments[1])
                            else:
                                raise ValueError("invalid ignore reason syntax")
                            found.add(
                                (
                                    source.relative_to(root).as_posix(),
                                    test_name,
                                    reason,
                                )
                            )
                else:
                    if child.type == "ERROR" and b"#" in child.text:
                        raise ValueError("invalid Rust attribute syntax")
                    visit(child)

        visit(tree.root_node)
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
    try:
        actual = _ignored_tests(root)
    except (OSError, ValueError) as error:
        return [f"ignored-test source scan failed: {error}"]
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
