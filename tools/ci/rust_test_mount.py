"""Resolve registered Rust integration modules through their actual launcher."""

from __future__ import annotations

import os
import re
from collections import deque
from collections.abc import Callable
from functools import cache
from pathlib import Path
from typing import TYPE_CHECKING, Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib  # type: ignore[no-redef]

from tools.ci.lint.handoff_validation import _read_repo_regular_bytes
from tools.ci.lint.rust_attribute_policy import attribute_metas

if TYPE_CHECKING:
    from tree_sitter import Node


def parse_test_mounts(text: str) -> tuple[list[tuple[str, str, str]], list[str]]:
    """Accept the grouped-launcher grammar shared by the catalog, selector and verifier."""
    mounts = []
    errors = []
    names = set()
    lines = text.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i].strip()
        i += 1
        if (
            not line
            or line.startswith("//")
            or line == "#![forbid(unsafe_code)]"
            or re.fullmatch(r"use\s+[A-Za-z_][\w:]*\s+as\s+[A-Za-z_]\w*;", line)
        ):
            continue
        cfg = ""
        if re.fullmatch(r"#\[\s*cfg\s*\(\s*debug_assertions\s*\)\s*\]", line):
            cfg = "debug_assertions"
            line = lines[i].strip() if i < len(lines) else ""
            i += 1
        source = re.fullmatch(r'#\[\s*path\s*=\s*"([A-Za-z0-9_./-]+)"\s*\]', line)
        module = (
            re.fullmatch(r"mod\s+([A-Za-z_]\w*);", lines[i].strip()) if i < len(lines) else None
        )
        if not source or not module:
            errors.append(f"has unsupported launcher syntax on line {i}")
            continue
        name = module[1]
        if name in names:
            errors.append(f"duplicates module {name}")
        names.add(name)
        mounts.append((source[1], name, cfg))
        i += 1
    return mounts, errors


def committed_path_resolver(
    git: Callable[..., bytes], root: Path, head: str
) -> Callable[[Path], Path | None]:
    """Resolve Rust paths in the recorded tree, including relative aliases and symlinks."""

    @cache
    def directory(path: Path) -> dict[str, tuple[bytes, bytes]]:
        tree = head if not path.parts else f"{head}:{path.as_posix()}"
        try:
            listing = git(root, "--no-replace-objects", "ls-tree", "-z", tree)
        except RuntimeError as error:
            raise ValueError("cannot read committed test directory") from error
        entries = {}
        for item in listing.split(b"\0"):
            if item:
                meta, name = item.split(b"\t", 1)
                mode, _, oid = meta.split()
                entries[os.fsdecode(name)] = (mode, oid)
        return entries

    @cache
    def resolve(path: Path) -> Path | None:
        pending = deque(path.parts)
        current = Path()
        links = 0
        while pending:
            name = pending.popleft()
            if name == "..":
                if not current.parts:
                    raise ValueError("grouped source escapes repository")
                current = current.parent
                continue
            entry = directory(current).get(name)
            if entry is None:
                return None
            mode, oid = entry
            if mode == b"120000":
                links += 1
                if links > 40:
                    raise ValueError("cyclic grouped test source")
                try:
                    target = Path(
                        os.fsdecode(
                            git(root, "--no-replace-objects", "cat-file", "blob", oid.decode())
                        )
                    )
                except RuntimeError as error:
                    raise ValueError("cannot read committed test symlink") from error
                if target.is_absolute():
                    raise ValueError("grouped source must stay inside its recorded checkout")
                pending.extendleft(reversed(target.parts))
            else:
                if pending and mode != b"040000":
                    return None
                current /= name
        return current

    return resolve


def mounted_test_module(
    root: Path,
    source: Path,
    owner: str,
    cargo_target: str,
    entries: list[dict[str, Any]],
    *,
    read_source: Callable[[str], bytes] | None = None,
    resolve_path: Callable[[Path], Path | None] | None = None,
) -> str:
    group = [
        entry
        for entry in entries
        if entry["owner"] == owner and entry.get("target", Path(entry["path"]).stem) == cargo_target
    ]
    launchers = [Path(entry["path"]) for entry in group if Path(entry["path"]).stem == cargo_target]
    member = source.relative_to(root)
    members = [entry for entry in group if Path(entry["path"]) == member]
    if len(launchers) != 1 or len(members) != 1 or member == launchers[0]:
        raise ValueError(f"module target {source} needs a separate cataloged launcher")
    launcher = launchers[0]
    member.relative_to(launcher.parent)
    if resolve_path is None:

        def resolve_path(path: Path) -> Path | None:
            try:
                return (root / path).resolve(strict=True).relative_to(root.resolve())
            except (FileNotFoundError, NotADirectoryError):
                return None

    if read_source is None:

        def read_source(name: str) -> bytes:
            return _read_repo_regular_bytes(root, name, label="grouped test source")

    resolved_launcher = resolve_path(launcher)
    resolved_member = resolve_path(member)
    if resolved_launcher is None or resolved_member is None:
        raise ValueError(f"module target {source} lacks its registered source or launcher")
    resolved_member.relative_to(resolved_launcher.parent)
    mounts, errors = parse_test_mounts(read_source(resolved_launcher.as_posix()).decode("utf-8"))
    if errors:
        raise ValueError(f"{launcher} {errors[0]}")
    matches = []
    for path, module, cfg in mounts:
        relative = Path(path)
        if relative.is_absolute():
            raise ValueError(f"{launcher} requires a relative source path: {path}")
        if resolve_path(launcher.parent / relative) == resolved_member:
            if cfg != members[0].get("cfg", ""):
                raise ValueError(f"{launcher} cfg for {path} differs from catalog")
            matches.append(module)
    if len(matches) != 1:
        raise ValueError(f"module target {source} is not mounted exactly once in {launcher}")
    read_source(resolved_member.as_posix())
    return matches[0]


def retirement_event_matcher(
    root: Path,
    read_source: Callable[[str], str],
    *,
    resolve_path: Callable[[Path], Path | None] | None = None,
) -> Callable[[Path, str, str], bool]:
    """Resolve the Nextest binary and module from the candidate test source."""

    @cache
    def integration_rows() -> dict[Path, dict]:
        rows = tomllib.loads(read_source("tools/ci/test-authority.toml"))["integration_targets"]
        return {Path(row["path"]): row for row in rows}

    @cache
    def event_owner(source: Path) -> tuple[str, tuple[str, ...]]:
        relative = source.relative_to(root)
        _, package, tree, *parts = relative.parts
        if tree == "src":
            library = tomllib.loads(read_source(f"crates/{package}/Cargo.toml")).get("lib", {})
            target = library.get("name", package.replace("-", "_"))
            leaf = Path(parts[-1]).stem
            modules = tuple(parts[:-1]) if leaf in ("lib", "mod") else (*parts[:-1], leaf)
            return f"{package}::{target}", modules

        rows = integration_rows()
        ancestors = [parent.with_suffix(".rs") for parent in relative.parents if parent != Path(".")]
        mounted = next((path for path in (relative, *ancestors) if path in rows), None)
        if mounted is None or rows[mounted]["owner"] != package:
            raise ValueError(f"{relative} lacks a registered integration test owner")
        target = rows[mounted].get("target", mounted.stem)
        modules = []
        if target != mounted.stem:
            modules.append(mounted_test_module(
                root, root / mounted, package, target, list(rows.values()),
                read_source=lambda name: read_source(name).encode("utf-8"),
                resolve_path=resolve_path,
            ))
        if mounted != relative:
            tail = relative.relative_to(mounted.with_suffix(""))
            parent = mounted
            for part in tail.parts:
                child = parent.with_suffix("") / part
                if child.suffix != ".rs":
                    child = child.with_suffix(".rs")
                mount_path = re.escape(child.relative_to(parent.parent).as_posix())
                matches = re.findall(
                    rf'(?m)^\s*#\[\s*path\s*=\s*"{mount_path}"\s*\]\s*mod\s+([A-Za-z_]\w*)\s*;',
                    read_source(parent.as_posix()),
                )
                if len(matches) != 1:
                    raise ValueError(f"{child} is not mounted exactly once in {parent}")
                modules.append(matches[0])
                parent = child
        return f"{package}::{target}", tuple(modules)

    @cache
    def library_tests(source: Path) -> dict[str, list[tuple[str, ...]]]:
        try:
            from tree_sitter_language_pack import get_parser
        except ImportError as error:
            raise ValueError("Rust parser is required to identify library test evidence") from error

        body = read_source(source.relative_to(root).as_posix()).encode("utf-8")
        syntax = get_parser("rust").parse(body).root_node
        if syntax.has_error:
            raise ValueError(f"{source}: cannot parse library test source")
        tests: dict[str, list[tuple[str, ...]]] = {}

        def visit(scope: Node, modules: tuple[str, ...]) -> None:
            is_test = False
            for item in scope.named_children:
                if item.type == "attribute_item":
                    is_test |= any(
                        name == "test" and not args and not conditional
                        for name, args, conditional in attribute_metas(item)
                    )
                    continue
                if item.type in {"line_comment", "block_comment"}:
                    continue
                if item.type in {"function_item", "mod_item"}:
                    identifier = item.child_by_field_name("name")
                    if identifier is None:
                        raise ValueError(f"{source}: cannot identify {item.type}")
                    name = identifier.text.decode("utf-8").removeprefix("r#")
                    if item.type == "function_item" and is_test:
                        tests.setdefault(name, []).append(modules)
                    if item.type == "mod_item":
                        nested = item.child_by_field_name("body")
                        if nested is not None:
                            visit(nested, (*modules, name))
                is_test = False

        visit(syntax, ())
        return tests

    def matches(path: Path, binary: str, name: str) -> bool:
        expected_binary, modules = event_owner(path)
        if binary != expected_binary:
            return False
        *actual_modules, leaf = name.split("::")
        if path.relative_to(root).parts[2] == "src":
            inline_modules = library_tests(path).get(leaf, [])
            return (
                len(inline_modules) == 1
                and tuple(actual_modules) == modules + inline_modules[0]
            )
        return tuple(actual_modules) == modules

    return matches
