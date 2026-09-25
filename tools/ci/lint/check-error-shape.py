#!/usr/bin/env python3
"""Force public `*Error` enums to implement `std::error::Error` cleanly.

A public type named `*Error` that does not implement `std::error::Error` is a
soft contract violation: downstream callers can no longer `?`-propagate it
through a typed Result chain without manual wrapping, which usually leads to
ad-hoc `Box<dyn Error>` or — worse — silent fallback (`.ok()`, `unwrap_or`).

Structural invariants (Rust AST, not a file-wide regex):

  1. Every `pub enum *Error` (or `pub struct *Error`) in workspace source must
     satisfy ONE of:
       (a) an attached `#[derive(...)]` attribute includes
           `Error` (the thiserror derive form), OR
       (b) the same lexical module contains a typed Error impl AND a typed
           Display impl for X (manual impl form — preferred in
           this repo because thiserror's proc-macro derive adds cold-build
           cost the build-hygiene policy wants to avoid).
  2. If form (a) is used, every variant of a `pub enum *Error` must carry its
     own `#[error("...")]` attribute. Form (b) hand-rolls Display, so
     per-variant attributes are not required.

Tests directories are excluded. Sealed / non-public error types are out of
scope; this is a public-surface gate.

Wire-protocol DTOs named `*Error` (carry-error-on-the-wire structs without
Rust-side Error semantics) are explicitly allowlisted via WIRE_DTO_ERRORS.
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
CRATES_DIR = ROOT / "crates"

ERROR_NAME_RE = re.compile(r"[A-Z][A-Za-z0-9_]*Error(?:V[0-9]+)?")
PARSER = get_parser("rust")

# Types named `*Error` that are NOT Rust error types — typically wire-protocol
# DTOs carrying error-shaped payloads across IPC. They do not need to implement
# std::error::Error because callers map them into a typed local Error first.
WIRE_DTO_ERRORS: frozenset[str] = frozenset(
    {
        # IPC envelope: `code` + `message` pair carried on the wire. Consumers
        # convert into a local Rust error before propagating.
        "SearchPlaneIpcError",
    }
)


@dataclass(frozen=True)
class Violation:
    path: Path
    line: int
    message: str


def workspace_members() -> list[Path]:
    data = tomllib.loads(WORKSPACE_TOML.read_text(encoding="utf-8"))
    return [ROOT / m for m in data.get("workspace", {}).get("members", [])]


def crate_source_files() -> list[Path]:
    files: list[Path] = []
    for member in workspace_members():
        src = member / "src"
        if not src.is_dir():
            continue
        for rs in sorted(src.rglob("*.rs")):
            if any(part == "tests" for part in rs.parts):
                continue
            files.append(rs)
    return files


def node_text(source: bytes, node: Any) -> str:
    return source[node.start_byte : node.end_byte].decode("utf-8")


def walk(node: Any):
    stack = [node]
    while stack:
        current = stack.pop()
        yield current
        stack.extend(reversed(current.named_children))


def attached_attributes(node: Any) -> list[Any]:
    siblings = node.parent.named_children
    index = next(index for index, sibling in enumerate(siblings) if sibling.id == node.id)
    attributes: list[Any] = []
    for previous in reversed(siblings[:index]):
        if previous.type != "attribute_item":
            break
        attributes.append(previous)
    return attributes


def attribute_name(source: bytes, node: Any) -> str | None:
    attribute = next((child for child in node.named_children if child.type == "attribute"), None)
    if attribute is None:
        return None
    name = next((child for child in attribute.named_children if child.type == "identifier"), None)
    return node_text(source, name) if name is not None else None


def has_error_derive(source: bytes, node: Any) -> bool:
    for item in attached_attributes(node):
        if attribute_name(source, item) != "derive":
            continue
        attribute = item.named_children[0]
        arguments = next(
            (child for child in attribute.named_children if child.type == "token_tree"), None
        )
        if arguments is not None and any(
            child.type == "identifier" and node_text(source, child) == "Error"
            for child in walk(arguments)
        ):
            return True
    return False


def manual_impls(source: bytes, declaration: Any, name: str) -> tuple[bool, bool]:
    error = display = False
    for sibling in declaration.parent.named_children:
        if sibling.type != "impl_item":
            continue
        target = sibling.child_by_field_name("type")
        trait = sibling.child_by_field_name("trait")
        if target is None or trait is None or node_text(source, target).removeprefix("r#") != name:
            continue
        trait_name = node_text(source, trait).removeprefix("::")
        if trait_name in {"std::error::Error", "core::error::Error"}:
            error = True
        if trait_name in {"Display", "fmt::Display", "std::fmt::Display", "core::fmt::Display"}:
            display = True
    return error, display


def audit_enum_variants(path: Path, source: bytes, declaration: Any, name: str) -> list[Violation]:
    body = declaration.child_by_field_name("body")
    if body is None:
        raise ValueError(f"{path}: enum {name} has no parseable body")
    return [
        Violation(
            path,
            variant.start_point.row + 1,
            f'variant `{name}::{node_text(source, variant.child_by_field_name("name")).removeprefix("r#")}` lacks `#[error("...")]` attribute',
        )
        for variant in body.named_children
        if variant.type == "enum_variant"
        if not any(attribute_name(source, item) == "error" for item in attached_attributes(variant))
    ]


def audit_file(path: Path) -> list[Violation]:
    source = path.read_bytes()
    if b"pub" not in source or b"Error" not in source:
        return []
    root = PARSER.parse(source).root_node
    for error in walk(root):
        if error.type != "ERROR" and not error.is_missing:
            continue
        line = error.start_point.row
        nearby = b"\n".join(source.splitlines()[max(0, line - 1) : line + 2])
        if b"pub" in nearby and b"Error" in nearby:
            raise ValueError(f"{path}:{line + 1}: Rust parse error overlaps public error syntax")
    findings: list[Violation] = []
    for node in walk(root):
        if node.type not in {"enum_item", "struct_item"}:
            continue
        visibility = next(
            (child for child in node.named_children if child.type == "visibility_modifier"), None
        )
        if visibility is None or node_text(source, visibility) != "pub":
            continue
        name_node = node.child_by_field_name("name")
        if name_node is None:
            raise ValueError(
                f"{path}:{node.start_point.row + 1}: public error name is not parseable"
            )
        name = node_text(source, name_node).removeprefix("r#")
        if not ERROR_NAME_RE.fullmatch(name) or name in WIRE_DTO_ERRORS:
            continue
        kind = "enum" if node.type == "enum_item" else "struct"
        derived = has_error_derive(source, node)
        manual_error, manual_display = manual_impls(source, node, name)
        if not derived and not (manual_error and manual_display):
            findings.append(
                Violation(
                    path,
                    node.start_point.row + 1,
                    f"`pub {kind} {name}` does not implement `std::error::Error`. "
                    "Either add an attached `#[derive(Debug, thiserror::Error)]` "
                    f"or typed Error and Display impls for {name} in the same module.",
                )
            )
        elif kind == "enum" and derived:
            findings.extend(audit_enum_variants(path, source, node, name))
    return findings


def main() -> int:
    try:
        violations: list[Violation] = []
        files = crate_source_files()
        for path in files:
            violations.extend(audit_file(path))
    except (OSError, ValueError) as error:
        print(f"Error-shape check blocked: {error}", file=sys.stderr)
        return 2

    if violations:
        print("Error-shape check failed:", file=sys.stderr)
        for v in violations:
            rel = v.path.relative_to(ROOT) if v.path.is_absolute() else v.path
            print(f"  - {rel}:{v.line}: {v.message}", file=sys.stderr)
        return 1

    print(f"All public *Error types in {len(files)} source files have shape OK.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
