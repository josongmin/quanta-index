"""Rust attribute syntax shared by derive, ignore and allow policy owners."""

from __future__ import annotations

import json


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

    def expand(tokens: list[object], conditional: bool) -> list[tuple[str, list[object], bool]]:
        tokens = [token for token in tokens if token.type not in {"line_comment", "block_comment"}]
        if not tokens or tokens[0].type != "identifier":
            return []
        name = tokens[0].text.decode("utf-8")
        if name != "cfg_attr":
            return [(name, tokens[1:], conditional)]
        if len(tokens) != 2 or tokens[1].type != "token_tree":
            raise ValueError("cfg_attr has no argument list")
        groups: list[list[object]] = [[]]
        for token in tokens[1].children[1:-1]:
            if token.type == ",":
                groups.append([])
            else:
                groups[-1].append(token)
        if len(groups) < 2:
            raise ValueError("cfg_attr has no output attribute")
        return [meta for group in groups[1:] for meta in expand(group, True)]

    return expand(list(attribute.children), False)


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
