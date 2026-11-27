#!/usr/bin/env python3
"""Force public `*Error` enums to implement `std::error::Error` cleanly.

A public type named `*Error` that does not implement `std::error::Error` is a
soft contract violation: downstream callers can no longer `?`-propagate it
through a typed Result chain without manual wrapping, which usually leads to
ad-hoc `Box<dyn Error>` or — worse — silent fallback (`.ok()`, `unwrap_or`).

Structural invariants (Rust AST, not a file-wide regex):

  1. Every `pub enum *Error` (or `pub struct *Error`) in first-party workspace
     source must
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

An exact complementary std/no_std pair with recognized Error trait paths is
accepted. A single gated impl does not prove coverage across both branches.

Tests directories and vendored dependency crates are excluded. Vendor crates
have their own feature and MSRV contracts; this syntactic first-party policy
cannot establish their Error trait aliases across configurations. Sealed /
non-public error types are out of scope; this is a public-surface gate.

Wire-protocol DTOs named `*Error` (carry-error-on-the-wire structs without
Rust-side Error semantics) are explicitly allowlisted via WIRE_DTO_ERRORS.
"""

from __future__ import annotations

import os
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

    def fail(error: OSError) -> None:
        raise error

    for member in workspace_members():
        if member.relative_to(ROOT).parts[:1] == ("vendor",):
            continue
        src = member / "src"
        if src.resolve() != src:
            raise ValueError(f"cannot certify symlinked Rust source: {src}")
        if not src.is_dir():
            continue
        for directory, directories, names in os.walk(src, onerror=fail):
            directories[:] = [name for name in directories if name != "tests"]
            for name in directories:
                path = Path(directory) / name
                if path.is_symlink():
                    raise ValueError(f"cannot certify symlinked Rust source: {path}")
            for name in names:
                if not name.endswith(".rs"):
                    continue
                path = Path(directory) / name
                if path.is_symlink():
                    raise ValueError(f"cannot certify symlinked Rust source: {path}")
                files.append(path)
    return sorted(files)


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
        if previous.type in {"line_comment", "block_comment"}:
            continue
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


def cfg_guards(source: bytes, node: Any) -> set[tuple[str, ...]]:
    """Compare Rust cfg token sequences without losing literal contents."""
    guards: set[tuple[str, ...]] = set()

    def leaves(current: Any):
        if current.type in {"line_comment", "block_comment"}:
            return
        if current.type in {"string_literal", "raw_string_literal"}:
            yield node_text(source, current)
            return
        if not current.children:
            yield node_text(source, current)
            return
        for child in current.children:
            yield from leaves(child)

    for item in attached_attributes(node):
        if attribute_name(source, item) not in {"cfg", "cfg_attr"}:
            continue
        guards.add(tuple(leaves(item)))
    return guards


def impl_covers_generics(source: bytes, declaration: Any, impl: Any, target: Any) -> bool:
    """A concrete or repeated argument covers only part of a public generic type."""
    declared = next(
        (child for child in declaration.named_children if child.type == "type_parameters"), None
    )
    if declared is None:
        return target.type != "generic_type"
    if target.type != "generic_type":
        return False
    arguments = next(
        (child for child in target.named_children if child.type == "type_arguments"), None
    )
    introduced = next(
        (child for child in impl.named_children if child.type == "type_parameters"), None
    )
    if arguments is None or introduced is None:
        return False
    parameter_kinds = {
        node_text(source, parameter.child_by_field_name("name")): parameter.type
        for parameter in introduced.named_children
        if parameter.child_by_field_name("name") is not None
    }
    declared_parameters = [
        child
        for child in declared.named_children
        if child.type not in {"line_comment", "block_comment"}
    ]
    supplied_arguments = [
        child
        for child in arguments.named_children
        if child.type not in {"line_comment", "block_comment"}
    ]
    if len(declared_parameters) != len(supplied_arguments):
        return False
    used: set[tuple[str, str]] = set()
    for parameter, argument in zip(declared_parameters, supplied_arguments):
        kind = parameter.type
        if kind not in {"type_parameter", "lifetime_parameter", "const_parameter"}:
            return False
        if kind == "lifetime_parameter" and argument.type != "lifetime":
            return False
        if kind != "lifetime_parameter" and argument.type != "type_identifier":
            return False
        name = node_text(source, argument)
        if parameter_kinds.get(name) != kind or (kind, name) in used:
            return False
        used.add((kind, name))
    return True


def manual_impls(source: bytes, declaration: Any, name: str) -> tuple[bool, bool]:
    error = display = False
    declaration_guards = cfg_guards(source, declaration)
    std_guard = ("#", "[", "cfg", "(", "feature", "=", '"std"', ")", "]")
    no_std_guard = (
        "#",
        "[",
        "cfg",
        "(",
        "not",
        "(",
        "feature",
        "=",
        '"std"',
        ")",
        ")",
        "]",
    )
    std_error = no_std_error = False
    for sibling in declaration.parent.named_children:
        if sibling.type != "impl_item":
            continue
        target = sibling.child_by_field_name("type")
        trait = sibling.child_by_field_name("trait")
        if target is None or trait is None:
            continue
        if target.type == "generic_type":
            target_name = next(
                (child for child in target.named_children if child.type == "type_identifier"),
                None,
            )
            if target_name is None:
                continue
            target = target_name
        if node_text(source, target).removeprefix("r#") != name:
            continue
        if not impl_covers_generics(
            source, declaration, sibling, sibling.child_by_field_name("type")
        ):
            continue
        trait_name = node_text(source, trait).removeprefix("::")
        error_trait = trait_name in {"std::error::Error", "core::error::Error"}
        display_trait = trait_name in {
            "Display",
            "fmt::Display",
            "std::fmt::Display",
            "core::fmt::Display",
        }
        if not error_trait and not display_trait:
            continue
        impl_guards = cfg_guards(source, sibling)
        # A production declaration cannot rely on a test-only (or otherwise
        # narrower) impl. Equivalent declaration/impl gates are permitted.
        if not impl_guards <= declaration_guards:
            if trait_name == "std::error::Error" and impl_guards == declaration_guards | {
                std_guard
            }:
                std_error = True
            elif trait_name == "core::error::Error" and impl_guards == declaration_guards | {
                no_std_guard
            }:
                no_std_error = True
            continue
        if error_trait:
            error = True
        if display_trait:
            display = True
    return error or (std_error and no_std_error), display


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
                    f"`pub {kind} {name}` Error and Display coverage cannot be established "
                    "for every enabled declaration configuration. "
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
