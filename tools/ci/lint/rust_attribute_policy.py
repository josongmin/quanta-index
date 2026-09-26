"""Rust attribute syntax shared by derive, ignore and allow policy owners."""

from __future__ import annotations

import json
import os
from pathlib import Path


def rust_source_files(roots: tuple[Path, ...]) -> list[Path]:
    """Inventory owned sources without silently suppressing traversal failures."""
    files: list[Path] = []

    def fail(error: OSError) -> None:
        raise error

    for root in roots:
        if not root.is_dir():
            raise ValueError(f"Rust source roots are missing: {root}")
        for directory, directories, names in os.walk(root, onerror=fail):
            parts = Path(directory).relative_to(root).parts
            # Cargo output at package/target or package/fuzz/target is outside
            # owned sources. A Rust module named target under src (or any other
            # source area) is ordinary input and must remain in the inventory.
            build_parent = len(parts) == 1 or (len(parts) == 2 and parts[-1] == "fuzz")
            source_area = any(
                part in {"src", "tests", "fuzz_targets", "benches", "examples"} for part in parts
            )
            if build_parent and not source_area:
                directories[:] = [name for name in directories if name != "target"]
            for name in directories:
                path = Path(directory) / name
                if path.is_symlink():
                    raise ValueError(f"cannot certify symlinked Rust source directory: {path}")
            files.extend(Path(directory) / name for name in names if name.endswith(".rs"))
    return sorted(files)


def _expand(tokens: list[object], conditional: bool) -> list[tuple[str, list[object], bool]]:
    tokens = [token for token in tokens if token.type not in {"line_comment", "block_comment"}]
    if tokens and tokens[0].type in {"scoped_identifier", "scoped_type_identifier"}:
        return [(tokens[0].text.decode("utf-8"), tokens[1:], conditional)]
    if not tokens or tokens[0].type != "identifier":
        raise ValueError("unsupported Rust attribute metadata")
    name = tokens[0].text.decode("utf-8").removeprefix("r#")
    if name != "cfg_attr":
        return [(name, tokens[1:], conditional)]
    if len(tokens) != 2 or tokens[1].type != "token_tree":
        raise ValueError("cfg_attr has no argument list")
    groups: list[list[object]] = [[]]
    for token in tokens[1].children[1:-1]:
        if token.type in {"line_comment", "block_comment"}:
            continue
        if token.type == ",":
            groups.append([])
        else:
            groups[-1].append(token)
    if len(groups) < 2:
        raise ValueError("cfg_attr has no output attribute")
    # Rust permits a trailing comma, but no empty output attribute.
    if not groups[-1]:
        groups.pop()
    if len(groups) < 2:
        raise ValueError("cfg_attr has no output attribute")
    return [meta for group in groups[1:] for meta in _expand(group, True)]


def attribute_metas(node: object) -> list[tuple[str, list[object], bool]]:
    """Expand attribute metadata, including nested cfg_attr, without its predicate.

    Comments, strings and arbitrary macro arguments never become attributes.
    Tree-sitter keeps nested token trees intact, so commas inside a predicate
    cannot be confused with the separator before cfg_attr's output attributes.
    """
    if node.has_error:
        raise ValueError("invalid Rust attribute syntax")
    attribute = next((child for child in node.named_children if child.type == "attribute"), None)
    if attribute is None:
        raise ValueError("Rust attribute has no metadata")

    return _expand(list(attribute.children), False)


def macro_attribute_metas(node: object) -> list[tuple[int, list[tuple[str, list[object], bool]]]]:
    """Inspect literal attributes in opaque macro token trees, never string contents."""
    found: list[tuple[int, list[tuple[str, list[object], bool]]]] = []

    def visit(parent: object) -> None:
        if parent.type in {
            "line_comment",
            "block_comment",
            "string_literal",
            "raw_string_literal",
            "token_tree_pattern",
        }:
            return
        children = [c for c in parent.children if c.type not in {"line_comment", "block_comment"}]
        for index, child in enumerate(children):
            if child.type == "#":
                following = children[index + 1 :]
                if following and following[0].type == "!":
                    following = following[1:]
                if (
                    following
                    and following[0].type == "token_tree"
                    and following[0].text.startswith(b"[")
                ):
                    tree = following[0]
                    if tree.has_error:
                        raise ValueError("invalid Rust macro attribute syntax")
                    tokens = list(tree.children[1:-1])
                    # A forwarded metavariable is not literal attribute metadata.
                    # Literal attributes at the call site are inspected separately.
                    if not any(token.type in {"$", "metavariable"} for token in tokens):
                        found.append((child.start_point.row + 1, _expand(tokens, False)))
            visit(child)

    visit(node)
    return found


def derive_names(arguments: list[object]) -> list[str]:
    """Read derive paths as tokens so comments and raw identifiers preserve meaning."""
    if len(arguments) != 1 or arguments[0].type != "token_tree":
        raise ValueError("derive attribute has no argument list")
    tokens = arguments[0].children[1:-1]
    groups: list[list[object]] = [[]]
    for token in tokens:
        if token.type in {"line_comment", "block_comment"}:
            continue
        if token.type == ",":
            groups.append([])
        else:
            groups[-1].append(token)
    if not groups[-1]:
        groups.pop()
    names: list[str] = []
    for group in groups:
        if not group or any(
            token.type not in {"identifier", "::", "crate", "self", "super"} for token in group
        ):
            raise ValueError("unsupported derive path syntax")
        names.append("".join(token.text.decode("utf-8").removeprefix("r#") for token in group))
    return names


def string_value(node: object) -> str:
    """Decode supported Rust string literals; unknown escapes fail closed."""
    if node.type == "raw_string_literal":
        content = next(
            (child for child in node.named_children if child.type == "string_content"), None
        )
        return "" if content is None else content.text.decode("utf-8")
    if node.type == "string_literal":
        value = json.loads(node.text.decode("utf-8"))
        if isinstance(value, str):
            return value
    raise ValueError("attribute reason must be a supported Rust string literal")
