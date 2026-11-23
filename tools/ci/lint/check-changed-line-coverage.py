#!/usr/bin/env python3
"""Fail closed when changed production Rust lines lack LCOV execution evidence."""

from __future__ import annotations

import argparse
import codecs
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

import tree_sitter_rust
from tree_sitter import Language, Parser

ROOT = Path(__file__).resolve().parents[3]
HUNK_RE = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
PURE_DECLARATIONS = frozenset(
    {
        "use_declaration",
        "extern_crate_declaration",
        "type_item",
        "struct_item",
        "enum_item",
        "union_item",
    }
)


@dataclass(frozen=True)
class CoverageResult:
    covered_lines: int
    total_lines: int
    violations: list[str]

    @property
    def percent(self) -> float | None:
        if self.total_lines == 0:
            return None
        return (self.covered_lines * 100.0) / self.total_lines


def _relative_source_path(root: Path, raw_path: str) -> str | None:
    source = Path(raw_path)
    try:
        relative = source.resolve().relative_to(root.resolve())
    except ValueError:
        return None
    return relative.as_posix()


def parse_lcov(root: Path, lcov_path: Path) -> dict[str, dict[int, int]]:
    """Read line hit counts keyed by repository-relative source path."""
    if not lcov_path.is_file():
        raise ValueError(f"LCOV artifact does not exist: {lcov_path}")

    coverage: dict[str, dict[int, int]] = {}
    current_source: str | None = None
    in_record = False
    for raw_line in lcov_path.read_text(encoding="utf-8").splitlines():
        if raw_line.startswith("SF:"):
            if in_record:
                raise ValueError("LCOV source record lacks end_of_record")
            current_source = _relative_source_path(root, raw_line.removeprefix("SF:"))
            in_record = True
            continue
        if raw_line == "end_of_record":
            if not in_record:
                raise ValueError("LCOV end_of_record has no source record")
            current_source = None
            in_record = False
            continue
        if not raw_line.startswith("DA:"):
            continue
        if not in_record:
            raise ValueError(f"LCOV DA record has no source: {raw_line!r}")
        payload = raw_line.removeprefix("DA:")
        try:
            line_text, hits_text, *_ = payload.split(",")
            line_number = int(line_text)
            hits = int(hits_text)
        except ValueError as error:
            raise ValueError(f"invalid LCOV DA record {raw_line!r}: {error}") from error
        if line_number <= 0 or hits < 0:
            raise ValueError(f"invalid LCOV DA record {raw_line!r}")
        if current_source is not None:
            lines = coverage.setdefault(current_source, {})
            if line_number in lines:
                raise ValueError(f"duplicate LCOV DA record: {current_source}:{line_number}")
            lines[line_number] = hits
    if in_record:
        raise ValueError("LCOV source record lacks end_of_record")
    return coverage


def _is_production_rust(path: str) -> bool:
    candidate = PurePosixPath(path)
    return (
        len(candidate.parts) >= 4
        and candidate.parts[0] == "crates"
        and candidate.parts[2] == "src"
        and candidate.suffix == ".rs"
    )


def _cfg_parts(group) -> list[list] | None:
    children = group.children
    if len(children) < 2 or children[0].type != "(" or children[-1].type != ")":
        return None
    parts: list[list] = [[]]
    for child in children[1:-1]:
        if child.type == ",":
            parts.append([])
        else:
            parts[-1].append(child)
    if parts[-1] == []:
        parts.pop()
    return parts


def _cfg_implies_test(nodes: list, source: bytes) -> bool:
    if len(nodes) == 1 and nodes[0].type == "identifier":
        return source[nodes[0].start_byte : nodes[0].end_byte] == b"test"
    if len(nodes) != 2 or nodes[0].type != "identifier" or nodes[1].type != "token_tree":
        return False
    operator = source[nodes[0].start_byte : nodes[0].end_byte]
    parts = _cfg_parts(nodes[1])
    if not parts or any(not part for part in parts):
        return False
    if operator == b"all":
        return any(_cfg_implies_test(part, source) for part in parts)
    if operator == b"any":
        return all(_cfg_implies_test(part, source) for part in parts)
    return False


def _test_only_attribute(node, source: bytes) -> bool:
    attributes = [child for child in node.named_children if child.type == "attribute"]
    if len(attributes) != 1:
        return False
    children = attributes[0].named_children
    if not children or children[0].type != "identifier":
        return False
    name = source[children[0].start_byte : children[0].end_byte]
    if name == b"test" and len(children) == 1:
        return True
    if name != b"cfg" or len(children) != 2 or children[1].type != "token_tree":
        return False
    parts = _cfg_parts(children[1])
    return parts is not None and len(parts) == 1 and _cfg_implies_test(parts[0], source)


def _eligible_changed_rust_lines(
    source: bytes, changed: set[int], path: str, inserted: set[int]
) -> set[int]:
    parser = Parser(Language(tree_sitter_rust.language()))
    root = parser.parse(source).root_node
    if root.has_error:
        raise ValueError(f"cannot classify changed Rust source: {path}")

    excluded = bytearray(len(source))

    def exclude(node) -> None:
        excluded[node.start_byte : node.end_byte] = b"\x01" * (node.end_byte - node.start_byte)

    def visit(parent) -> None:
        pending_attributes = []
        for child in parent.children:
            if child.type in {"line_comment", "block_comment"}:
                exclude(child)
                continue
            if child.type == "attribute_item":
                pending_attributes.append(child)
                continue
            if not child.is_named:
                continue
            test_guards = [
                attribute
                for attribute in pending_attributes
                if _test_only_attribute(attribute, source)
            ]
            guard_changed = any(
                number in changed
                for attribute in test_guards
                for number in range(attribute.start_point.row + 1, attribute.end_point.row + 2)
            )
            last_item_line = child.end_point.row + int(child.end_point.column > 0)
            item_inserted = all(
                number in inserted
                for number in range(child.start_point.row + 1, last_item_line + 1)
            )
            test_only = bool(test_guards) and (not guard_changed or item_inserted)
            pure_declaration = child.type in PURE_DECLARATIONS or (
                child.type == "mod_item"
                and not any(member.type == "declaration_list" for member in child.named_children)
            )
            if test_only or pure_declaration:
                for attribute in pending_attributes:
                    exclude(attribute)
                exclude(child)
            else:
                visit(child)
            pending_attributes.clear()

    visit(root)
    eligible: set[int] = set()
    offset = 0
    lines = source.split(b"\n") if source else []
    if lines and lines[-1] == b"":
        lines.pop()
    for number, line in enumerate(lines, 1):
        if number in changed:
            remaining = bytes(
                byte for index, byte in enumerate(line, offset) if not excluded[index]
            ).strip()
            if remaining:
                eligible.add(number)
        offset += len(line) + 1
    if changed and max(changed) > len(lines):
        raise ValueError(f"changed Rust line exceeds committed source: {path}")
    return eligible


def changed_production_lines(root: Path, base: str) -> set[tuple[str, int]]:
    """Return changed production code lines from the exact committed HEAD."""
    resolved = subprocess.run(
        ["git", "rev-parse", "--verify", "HEAD^{commit}"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    head = resolved.stdout.strip()
    if resolved.returncode != 0 or not re.fullmatch(r"[0-9a-f]{40,64}", head):
        raise ValueError("cannot resolve coverage candidate HEAD")
    result = subprocess.run(
        [
            "git",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--text",
            "--no-color",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--unified=0",
            f"{base}...{head}",
            "--",
            "crates",
        ],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ValueError(
            f"cannot diff coverage base {base!r}: {result.stderr.strip() or result.stdout.strip()}"
        )

    changed: dict[str, set[int]] = {}
    inserted: dict[str, set[int]] = {}
    current_path: str | None = None
    destination_pending = False
    for line in result.stdout.split("\n"):
        if line.startswith("diff --git "):
            current_path = None
            destination_pending = True
            continue
        if destination_pending and line.startswith("+++ "):
            destination_pending = False
            destination = line.removeprefix("+++ ").removesuffix("\t")
            if destination.startswith('"'):
                if not destination.endswith('"'):
                    raise ValueError("unterminated Git destination path")
                # Git C-quotes tabs, quotes, and UTF-8 bytes (as octal), even
                # when core.quotepath is false for other Unicode characters.
                destination = codecs.escape_decode(destination[1:-1].encode("utf-8"))[0].decode(
                    "utf-8"
                )
            if destination == "/dev/null":
                current_path = None
                continue
            if not destination.startswith("b/"):
                raise ValueError(f"unexpected Git destination path: {destination!r}")
            candidate = destination[2:]
            current_path = candidate if _is_production_rust(candidate) else None
            continue
        match = HUNK_RE.match(line)
        if match is None or current_path is None:
            continue
        old_count = int(match.group(2) or "1")
        start = int(match.group(3))
        count = int(match.group(4) or "1")
        if count:
            lines = range(start, start + count)
            changed.setdefault(current_path, set()).update(lines)
            if old_count == 0:
                inserted.setdefault(current_path, set()).update(lines)
    eligible: set[tuple[str, int]] = set()
    for path, lines in changed.items():
        committed = subprocess.run(
            ["git", "show", f"{head}:{path}"], cwd=root, check=False, capture_output=True
        )
        if committed.returncode != 0:
            raise ValueError(f"cannot read committed Rust source: {path}")
        eligible.update(
            (path, line_number)
            for line_number in _eligible_changed_rust_lines(
                committed.stdout,
                lines,
                path,
                inserted.get(path, set()),
            )
        )
    return eligible


def audit_changed_coverage(
    root: Path,
    lcov_path: Path,
    changed_lines: set[tuple[str, int]],
    minimum_percent: float,
) -> CoverageResult:
    """Check changed lines against LCOV; absent evidence is a violation."""
    if not 0.0 <= minimum_percent <= 100.0:
        raise ValueError("minimum percent must be within 0..=100")

    coverage = parse_lcov(root, lcov_path)
    violations: list[str] = []
    uncovered: list[str] = []
    covered = 0
    for path, line_number in sorted(changed_lines):
        file_coverage = coverage.get(path)
        if file_coverage is None:
            violations.append(f"changed source missing from LCOV: {path}")
            continue
        hits = file_coverage.get(line_number)
        if hits is None:
            violations.append(f"changed line missing from LCOV: {path}:{line_number}")
            continue
        if hits == 0:
            uncovered.append(f"uncovered changed line: {path}:{line_number}")
            continue
        covered += 1

    total = len(changed_lines)
    result = CoverageResult(covered_lines=covered, total_lines=total, violations=violations)
    if result.percent is not None and result.percent < minimum_percent:
        violations.extend(uncovered)
        violations.append(
            f"changed-line coverage {result.percent:.2f}% is below required {minimum_percent:.2f}%"
        )
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lcov", type=Path, required=True, help="LCOV artifact produced for HEAD")
    parser.add_argument("--base", required=True, help="Git revision used as the changed-line base")
    parser.add_argument("--minimum-percent", type=float, default=90.0)
    args = parser.parse_args()

    try:
        changed = changed_production_lines(ROOT, args.base)
        result = audit_changed_coverage(ROOT, args.lcov, changed, args.minimum_percent)
    except ValueError as error:
        print(f"changed-line coverage gate error: {error}", file=sys.stderr)
        return 2

    if result.percent is None:
        print("changed-line coverage: no changed production Rust lines; no coverage claim made")
        return 0

    print(
        "changed-line coverage: "
        f"{result.covered_lines}/{result.total_lines} ({result.percent:.2f}%), "
        f"required {args.minimum_percent:.2f}%"
    )
    for violation in result.violations:
        print(f"coverage violation: {violation}", file=sys.stderr)
    return 1 if result.violations else 0


if __name__ == "__main__":
    raise SystemExit(main())
