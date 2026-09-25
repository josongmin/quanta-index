#!/usr/bin/env python3
"""Reject public digest-array returns without an explicit failure channel.

The Rust AST owns function, return-type, attribute, and comment boundaries.
Only literal digest widths are governed; const-sized arrays remain out of scope.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from tree_sitter_language_pack import get_parser

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
WORKSPACE_TOML = ROOT / "Cargo.toml"
DIGEST_SIZES = frozenset({16, 20, 32, 48, 64})
PARSER = get_parser("rust")


@dataclass(frozen=True)
class Violation:
    path: Path
    line: int
    message: str


@dataclass(frozen=True)
class DigestSite:
    path: Path
    line: int


def workspace_members() -> list[Path]:
    data = tomllib.loads(WORKSPACE_TOML.read_text(encoding="utf-8"))
    return [ROOT / member for member in data.get("workspace", {}).get("members", [])]


def crate_source_files() -> list[Path]:
    files: list[Path] = []
    for member in workspace_members():
        src = member / "src"
        if not src.is_dir():
            continue
        files.extend(path for path in sorted(src.rglob("*.rs")) if "tests" not in path.parts)
    return files


def node_text(source: bytes, node: Any) -> str:
    return source[node.start_byte : node.end_byte].decode("utf-8")


def walk(node: Any):
    stack = [node]
    while stack:
        current = stack.pop()
        yield current
        stack.extend(reversed(current.named_children))


def preceding_metadata(node: Any):
    if node.parent is None:
        return
    siblings = node.parent.named_children
    index = next(index for index, sibling in enumerate(siblings) if sibling.id == node.id)
    for sibling in reversed(siblings[:index]):
        if sibling.type not in {"attribute_item", "line_comment"}:
            break
        yield sibling


def attribute_name(source: bytes, node: Any) -> str | None:
    attribute = next((child for child in node.named_children if child.type == "attribute"), None)
    if attribute is None:
        return None
    name = next((child for child in attribute.named_children if child.type == "identifier"), None)
    return node_text(source, name) if name is not None else None


def test_only(node: Any, source: bytes) -> bool:
    for current in (node, *list(ancestors(node))):
        if current.type == "mod_item":
            name = current.child_by_field_name("name")
            if name is not None and node_text(source, name) == "tests":
                return True
        for item in preceding_metadata(current):
            if item.type != "attribute_item":
                continue
            name = attribute_name(source, item)
            if name == "test":
                return True
            if name == "cfg" and re.fullmatch(
                r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", node_text(source, item)
            ):
                return True
    return False


def ancestors(node: Any):
    current = node.parent
    while current is not None:
        yield current
        current = current.parent


def infallible_doc(node: Any, source: bytes) -> bool:
    for item in preceding_metadata(node):
        if item.type == "attribute_item":
            continue
        comment = node_text(source, item)
        if not comment.startswith(("///", "//!")):
            break
        if "infallible by construction" in comment.lower():
            return True
    return False


def array_width(node: Any, source: bytes) -> int | None:
    if node.type != "array_type":
        return None
    parts = node.named_children
    if len(parts) != 2 or node_text(source, parts[0]) != "u8":
        return None
    if parts[1].type != "integer_literal":
        return None
    try:
        width = int(node_text(source, parts[1]).replace("_", ""))
    except ValueError:
        return None
    return width if width in DIGEST_SIZES else None


def digest_return(node: Any, source: bytes) -> tuple[int, bool] | None:
    result = node.child_by_field_name("return_type")
    if result is None:
        return None
    width = array_width(result, source)
    if width is not None:
        return width, False
    if result.type != "generic_type":
        return None
    base = result.child_by_field_name("type")
    args = result.child_by_field_name("type_arguments")
    if (
        base is None
        or args is None
        or node_text(source, base).removeprefix("::")
        not in {"Result", "std::result::Result", "core::result::Result"}
    ):
        return None
    first = next(iter(args.named_children), None)
    width = array_width(first, source) if first is not None else None
    return (width, True) if width is not None else None


def audit_file(path: Path) -> tuple[list[DigestSite], list[Violation]]:
    source = path.read_bytes()
    if b"pub" not in source or b"u8" not in source:
        return [], []
    root = PARSER.parse(source).root_node
    for node in walk(root):
        if (
            node.type == "ERROR"
            and b"pub" in source[node.start_byte : node.end_byte]
            and b"u8" in source[node.start_byte : node.end_byte]
        ):
            raise ValueError(
                f"{path}:{node.start_point.row + 1}: Rust parse error overlaps public digest syntax"
            )
    sites: list[DigestSite] = []
    findings: list[Violation] = []
    for node in walk(root):
        if node.type != "function_item":
            continue
        if not any(child.type == "visibility_modifier" for child in node.named_children):
            continue
        result = digest_return(node, source)
        if result is None:
            continue
        width, wrapped = result
        line = node.start_point.row + 1
        sites.append(DigestSite(path, line))
        if wrapped or test_only(node, source) or infallible_doc(node, source):
            continue
        findings.append(
            Violation(
                path,
                line,
                f"pub fn returning [u8; {width}] must return Result<[u8; {width}], _> "
                'or be documented "infallible by construction"',
            )
        )
    return sites, findings


def main() -> int:
    try:
        files = crate_source_files()
        scanned = [audit_file(path) for path in files]
    except (OSError, ValueError) as error:
        print(f"digest fallibility lint blocked: {error}", file=sys.stderr)
        return 2
    sites = [site for file_sites, _ in scanned for site in file_sites]
    violations = [item for _, file_violations in scanned for item in file_violations]
    for item in violations:
        relative = item.path.relative_to(ROOT) if item.path.is_absolute() else item.path
        print(f"{relative}:{item.line}: {item.message}", file=sys.stderr)
    print(
        f"Scanned {len(files)} rs files, found {len(sites)} digest-shape "
        f"return sites, {len(violations)} violations."
    )
    return 1 if violations else 0


if __name__ == "__main__":
    raise SystemExit(main())
