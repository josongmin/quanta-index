#!/usr/bin/env python3
"""Structural guard for the S21-05 read-view cutover (SEP-21 P04).

Two invariants, checked statically so they cannot regress silently:

1. Zero ambient lookup after view acquisition: a query route may reach a
   backend only through the view's accessors. The routes directory must
   not name the dispatcher's registries, openers, the RepoMap acquire
   port, or the ledger; those are acquisition-time collaborators that
   live in ``read_view/`` only.
2. Zero V1 live path: the production sources must not name the retired
   ``QueryReadViewV1`` / ``ReadIdentityV1`` / ``RepoMapQueryPort`` /
   ``read_query_snapshot`` ambient surfaces anywhere in the workspace.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

ROUTE_FORBIDDEN = (
    "repo_map_snapshots",
    "snapshots",
    "lex_opener",
    "sem_opener",
    "ledger",
    "history_text",
)

WORKSPACE_FORBIDDEN = (
    "QueryReadViewV1",
    "ReadIdentityV1",
    "RepoMapQueryPort",
    "read_query_snapshot",
)
RAW_STRING = re.compile(r'(?:b|c)?r(#{0,255})"')
CHARACTER = re.compile(r"'(?:\\.|[^'\\\n])'")
TOKEN_START = re.compile(r'//|/\*|(?:b|c)?r#{0,255}"|(?:b|c)?"|\'')
WRAPPED_SELF = re.compile(r"\(\s*(?:(?:&\s*(?:mut\s+)?)|(?:\*\s*))*self\s*\)")
PATH_ATTRIBUTE = re.compile(r'#\s*\[\s*path\s*=\s*"([^"\\]+)"\s*\]')
ROUTE_PATTERNS = {
    field: re.compile(rf"(?<![\w])self\s*\.\s*(?:r#)?{field}\b") for field in ROUTE_FORBIDDEN
}
ROUTE_FIELD_PATTERNS = {
    field: re.compile(rf"\.\s*(?:r#)?{field}\b(?!\s*\()") for field in ROUTE_FORBIDDEN
}
WORKSPACE_PATTERNS = {
    identifier: re.compile(rf"\b{identifier}\b") for identifier in WORKSPACE_FORBIDDEN
}


def code_without_comments_or_literals(source: str) -> str:
    """Mask Rust comments and literals while preserving token spacing and line numbers."""
    output = list(source)
    index = 0
    length = len(source)

    def mask(start: int, end: int) -> None:
        output[start:end] = re.sub(r"[^\n]", " ", source[start:end])

    while index < length:
        token = TOKEN_START.search(source, index)
        if token is None:
            break
        index = token.start()
        if source.startswith("//", index):
            end = source.find("\n", index)
            end = length if end < 0 else end
            mask(index, end)
            index = end
            continue
        if source.startswith("/*", index):
            start = index
            depth = 1
            index += 2
            while index < length and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            if depth:
                raise ValueError("unterminated Rust block comment")
            mask(start, index)
            continue
        raw = RAW_STRING.match(source, index)
        if raw and (index == 0 or not (source[index - 1].isalnum() or source[index - 1] == "_")):
            delimiter = '"' + raw.group(1)
            end = source.find(delimiter, raw.end())
            if end < 0:
                raise ValueError("unterminated Rust raw string")
            end += len(delimiter)
            mask(index, end)
            index = end
            continue
        prefix = 1 if source[index] in "bc" and source[index + 1 : index + 2] == '"' else 0
        if source[index] == '"' or prefix:
            start = index
            index += prefix + 1
            while index < length:
                if source[index] == "\\":
                    index += 2
                elif source[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            else:
                raise ValueError("unterminated Rust string")
            mask(start, min(index, length))
            continue
        if source[index] == "'":
            character = CHARACTER.match(source, index)
            if character:
                mask(index, character.end())
                index = character.end()
                continue
        index += 1
    return "".join(output)


def normalize_self_receivers(code: str) -> str:
    """Collapse simple parenthesized borrow/deref forms before field matching."""
    while True:
        normalized = WRAPPED_SELF.sub("self", code)
        if normalized == code:
            return code
        code = normalized


def route_module_inventory(
    routes: Path, trees: dict, route_files: set[Path], root: Path
) -> list[str]:
    """Follow route declarations using Rust's default and explicit path bases."""
    violations: list[str] = []
    visited: set[tuple[Path, Path]] = set()
    route_root = routes.resolve()

    def visit_file(path: Path, directory: Path) -> None:
        entry = path, directory
        if entry in visited or path not in trees:
            return
        visited.add(entry)
        visit(path, trees[path].root_node, directory, path.parent)

    def visit(path: Path, scope, directory: Path, literal_base: Path) -> None:
        source = path.relative_to(root)
        attributes = []
        for node in scope.named_children:
            if node.type == "attribute_item":
                attributes.append(node)
                continue
            if node.type != "mod_item":
                if node.type != "line_comment" and node.type != "block_comment":
                    attributes.clear()
                continue

            name_node = next(
                (child for child in node.named_children if child.type == "identifier"), None
            )
            if name_node is None:
                violations.append(f"{source} has an unsupported route module declaration")
                attributes.clear()
                continue
            name = name_node.text.decode("utf-8").removeprefix("r#")
            body = next(
                (child for child in node.named_children if child.type == "declaration_list"), None
            )
            override = None
            invalid_path = False
            for attribute in attributes:
                text = attribute.text.decode("utf-8")
                if re.match(r"#\s*\[\s*path\b", text):
                    match = PATH_ATTRIBUTE.fullmatch(text)
                    if not match or override is not None:
                        invalid_path = True
                    else:
                        override = match.group(1)
                elif re.match(r"#\s*\[\s*cfg_attr\b", text) and re.search(r"\bpath\b", text):
                    invalid_path = True
            attributes.clear()

            explicit = (literal_base / override).resolve() if override is not None else None
            if invalid_path or (
                explicit is not None
                and (Path(override).is_absolute() or not explicit.is_relative_to(route_root))
            ):
                violations.append(f"{source} declares unsupported path for route module {name}")
                continue
            if body is not None:
                child_dir = explicit if explicit is not None else directory / name
                visit(path, body, child_dir, child_dir)
                continue
            candidates = (
                (explicit,)
                if explicit is not None
                else (directory / f"{name}.rs", directory / name / "mod.rs")
            )
            found = [candidate for candidate in candidates if candidate in route_files]
            if len(found) != 1:
                issue = "missing" if not found else "ambiguous"
                violations.append(f"{source} declares {issue} route module {name}")
                continue
            child = found[0]
            child_dir = (
                child.parent if explicit is not None or child.name == "mod.rs" else directory / name
            )
            visit_file(child, child_dir)

    entry = routes / "mod.rs"
    if entry in route_files:
        visit_file(entry, routes)
    else:
        violations.append(f"{entry.relative_to(root)} is missing from route inventory")
    return violations


def scan(root: Path) -> list[str]:
    routes = root / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    sources = root / "crates"
    if not routes.is_dir() or not sources.is_dir():
        return ["required Rust source directory is missing"]
    route_files = set(routes.rglob("*.rs"))
    source_files = sorted(sources.glob("*/src/**/*.rs"))
    if not route_files or not source_files:
        return ["required Rust source scan is empty"]
    try:
        from tree_sitter_language_pack import get_parser

        route_parser = get_parser("rust")
    except Exception as error:
        return [f"Rust route parser is unavailable: {error}"]

    violations: list[str] = []
    route_trees = {}
    for path in sorted(route_files.difference(source_files)):
        violations.append(f"{path.relative_to(root)} is outside the Rust source scan")
    for path in source_files:
        try:
            raw = path.read_text(encoding="utf-8")
            if not raw:
                raise ValueError("empty Rust source")
            code = code_without_comments_or_literals(raw)
        except (OSError, UnicodeError, ValueError) as error:
            violations.append(f"{path.relative_to(root)} cannot be scanned: {error}")
            continue
        if path in route_files:
            encoded = raw.encode("utf-8")
            tree = route_parser.parse(encoded)
            if tree.root_node.has_error:
                violations.append(f"{path.relative_to(root)} cannot be parsed as Rust")
            else:
                route_trees[path] = tree
            route_code = normalize_self_receivers(code)
            for field, pattern in ROUTE_PATTERNS.items():
                if pattern.search(route_code):
                    violations.append(f"{path.relative_to(root)} names self.{field}")
                elif ROUTE_FIELD_PATTERNS[field].search(route_code):
                    violations.append(f"{path.relative_to(root)} names route field .{field}")
        for identifier, pattern in WORKSPACE_PATTERNS.items():
            if pattern.search(code):
                violations.append(f"{path.relative_to(root)} names retired surface {identifier}")
    return violations + route_module_inventory(routes, route_trees, route_files, root)


def fail(messages: list[str]) -> int:
    for message in messages:
        print(f"check-read-view-ambient-lookup: {message}", file=sys.stderr)
    print(
        f"check-read-view-ambient-lookup: {len(messages)} violation(s); "
        "routes must execute only through the read view and the V1 "
        "ambient surfaces must stay deleted",
        file=sys.stderr,
    )
    return 1


def main() -> int:
    violations = scan(ROOT)
    if violations:
        return fail(violations)
    print("check-read-view-ambient-lookup: ok (routes view-only, V1 surfaces absent)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
