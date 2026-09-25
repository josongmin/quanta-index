#!/usr/bin/env python3
"""Reject syntactic Rust Result fallback shapes in production source.

This is deliberately a syntax guard, not a type/flow proof. The three shapes
were previously owned by Semgrep; one Rust parse now checks all of them.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
RULE_OR_ELSE = "rust-no-silent-or-else-ok"
RULE_IS_OK = "rust-no-is-ok-as-branch"
RULE_IS_ERR = "rust-no-is-err-as-branch"
TOKEN_RE = re.compile(rb"\b(?:or_else|is_ok|is_err)\b")


def in_scope(path: Path) -> bool:
    parts = path.parts
    if path.suffix != ".rs" or any(part in {"tests", "benches"} for part in parts):
        return False
    if path.name == "tests.rs":
        return False
    if len(parts) >= 4 and parts[0] == "crates" and parts[2] == "src":
        return True
    return (
        len(parts) >= 5 and parts[0] == "crates" and parts[2:4] == ("fuzz", "fuzz_targets")
    ) or (len(parts) >= 4 and parts[0] == "benchmarks" and parts[2] == "src")


def text(source: bytes, node: Any) -> bytes:
    return source[node.start_byte : node.end_byte]


def semantic_children(node: Any) -> list[Any]:
    return [
        child
        for child in node.named_children
        if child.type not in {"line_comment", "block_comment"}
    ]


def unwrap_parens(node: Any) -> Any | None:
    while node is not None and node.type == "parenthesized_expression":
        children = semantic_children(node)
        node = children[0] if len(children) == 1 else None
    return node


def unwrap_generic_function(node: Any) -> Any | None:
    while node is not None and node.type == "generic_function":
        node = node.child_by_field_name("function")
    return node


def path_segments(source: bytes, part: Any) -> tuple[bytes, ...]:
    if part.type in {"identifier", "type_identifier"}:
        return (text(source, part),)
    if part.type == "generic_type":
        base = part.child_by_field_name("type")
        return path_segments(source, base) if base is not None else ()
    if part.type in {"scoped_identifier", "scoped_type_identifier"}:
        base = part.child_by_field_name("path")
        final = part.child_by_field_name("name")
        if base is not None and final is not None:
            return path_segments(source, base) + (text(source, final),)
    return ()


def call_name(source: bytes, node: Any, name: bytes) -> bool:
    node = unwrap_parens(node)
    if node is None or node.type != "call_expression":
        return False
    function = unwrap_generic_function(node.child_by_field_name("function"))
    if function is None:
        return False
    if function.type == "identifier":
        return text(source, function) == name
    if function.type != "scoped_identifier":
        return False
    variant = function.child_by_field_name("name")
    path = function.child_by_field_name("path")
    if variant is None or path is None or text(source, variant) != name:
        return False

    # Only the standard Result::Ok constructor is a success conversion.
    # A domain enum's Foo::Ok must not be interpreted as Result::Ok.
    return path_segments(source, path) in {
        (b"Result",),
        (b"std", b"result", b"Result"),
        (b"core", b"result", b"Result"),
    }


def method_call(source: bytes, node: Any, name: bytes) -> Any | None:
    node = unwrap_parens(node)
    if node is None or node.type != "call_expression":
        return None
    function = unwrap_generic_function(node.child_by_field_name("function"))
    arguments = node.child_by_field_name("arguments")
    if function is None or function.type != "field_expression" or arguments is None:
        return None
    field = function.child_by_field_name("field")
    if field is None or text(source, field) != name:
        return None
    if name != b"or_else" and semantic_children(arguments):
        return None
    return function.child_by_field_name("value")


def checked_width(source: bytes, receiver: Any) -> bool:
    if receiver is None or receiver.type != "call_expression":
        return False
    function = receiver.child_by_field_name("function")
    if function is None or function.type != "scoped_identifier":
        return False
    path = function.child_by_field_name("path")
    name = function.child_by_field_name("name")
    return (
        path is not None
        and name is not None
        and text(source, path) in {b"u8", b"u16", b"u32"}
        and text(source, name) == b"try_from"
    )


def closure_returns_ok(source: bytes, closure: Any) -> bool:
    closure = unwrap_parens(closure)
    if closure is not None and closure.type == "block":
        children = semantic_children(closure)
        closure = unwrap_parens(children[-1]) if children else None
    if closure is None:
        return False
    if closure.type != "closure_expression":
        return False
    body = closure.child_by_field_name("body")
    if body is None:
        return False
    pending = [body]
    while pending:
        node = pending.pop()
        if node is not body and node.type in {
            "closure_expression",
            "function_item",
            "async_block",
        }:
            continue
        if node.type == "return_expression":
            children = semantic_children(node)
            if children and call_name(source, children[0], b"Ok"):
                return True
        pending.extend(node.named_children)
    if body.type == "block":
        children = semantic_children(body)
        if not children:
            return False
        body = children[-1]
    return call_name(source, body, b"Ok")


def opaque_macro_contains_policy_syntax(source: bytes, macro: Any) -> bool:
    # A token tree is not a Rust expression AST. Inspect actual identifier/if
    # tokens, not raw bytes, so strings and comments do not create blockers.
    stack = list(macro.named_children[1:])
    has_if = False
    has_else = False
    has_predicate = False
    while stack:
        node = stack.pop()
        if node.type in {"line_comment", "block_comment", "string_literal"}:
            continue
        if node.type == "identifier":
            token = text(source, node)
            if token == b"or_else":
                return True
            if token in {b"is_ok", b"is_err"}:
                has_predicate = True
            if token == b"else":
                has_else = True
        elif node.type == "if":
            has_if = True
        stack.extend(node.children)
    return has_if and has_else and has_predicate


def scan_source(source: bytes, parser: Any) -> list[tuple[int, str]]:
    if not TOKEN_RE.search(source):
        return []
    root = parser.parse(source).root_node
    if root.has_error:
        # The pinned parser predates some valid Rust macro/token forms. Reject
        # errors touching governed syntax, but do not block unrelated syntax.
        lines = source.splitlines()
        errors = [root]
        while errors:
            node = errors.pop()
            if node.type == "ERROR" or node.is_missing:
                row = node.start_point.row
                nearby = b"\n".join(lines[max(row - 1, 0) : row + 1])
                if TOKEN_RE.search(text(source, node)) or TOKEN_RE.search(nearby):
                    raise ValueError(f"Rust parse error over fallback syntax at line {row + 1}")
            if node.has_error:
                errors.extend(node.children)
    findings: list[tuple[int, str]] = []
    stack = [root]
    while stack:
        node = stack.pop()
        if node.type in {"macro_invocation", "macro_definition"}:
            if opaque_macro_contains_policy_syntax(source, node):
                raise ValueError(
                    f"fallback syntax inside opaque Rust macro at line {node.start_point.row + 1}"
                )
            continue
        stack.extend(reversed(node.named_children))
        if node.type == "if_expression":
            alternative = node.child_by_field_name("alternative")
            if alternative is None or not any(
                child.type in {"block", "if_expression"} for child in alternative.named_children
            ):
                continue
            condition = node.child_by_field_name("condition")
            if method_call(source, condition, b"is_err") is not None:
                findings.append((node.start_point.row + 1, RULE_IS_ERR))
            elif (receiver := method_call(source, condition, b"is_ok")) is not None:
                if not checked_width(source, receiver):
                    findings.append((node.start_point.row + 1, RULE_IS_OK))
        elif node.type == "call_expression" and method_call(source, node, b"or_else") is not None:
            arguments = node.child_by_field_name("arguments")
            if arguments is not None:
                children = semantic_children(arguments)
                if len(children) == 1 and closure_returns_ok(source, children[0]):
                    findings.append((node.start_point.row + 1, RULE_OR_ELSE))
    return sorted(findings)


def main() -> int:
    try:
        from tree_sitter_language_pack import get_parser

        listed = subprocess.run(
            ["git", "ls-files", "-z", "--", "crates", "benchmarks"],
            cwd=ROOT,
            check=True,
            capture_output=True,
        ).stdout
        parser = get_parser("rust")
        findings = []
        scoped = 0
        candidates = 0
        for raw in listed.split(b"\0"):
            if not raw:
                continue
            relative = Path(raw.decode("utf-8"))
            if in_scope(relative):
                scoped += 1
                try:
                    source = (ROOT / relative).read_bytes()
                    if TOKEN_RE.search(source):
                        candidates += 1
                    for line, rule in scan_source(source, parser):
                        findings.append((relative, line, rule))
                except ValueError as error:
                    raise ValueError(f"{relative}: {error}") from error
    except (
        ImportError,
        OSError,
        RuntimeError,
        UnicodeError,
        ValueError,
        subprocess.CalledProcessError,
    ) as error:
        print(f"Rust fallback lint blocked: {error}", file=sys.stderr)
        return 2
    for path, line, rule in findings:
        print(f"{path}:{line}: {rule}")
    if findings:
        return 1
    print(f"Rust fallback lint: clean ({scoped} scoped files, {candidates} parsed candidates)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
