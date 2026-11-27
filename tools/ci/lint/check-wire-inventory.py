#!/usr/bin/env python3
"""Keep `tools/ci/inventory/wire-surface.toml` exact against the code.

The inventory is the W1 consumer / wire inventory plan §11 requires before
any state or wire cutover: every IPC opcode and every on-disk artifact
format, with its version constant and its reproduction class. An inventory
that drifts from the code is worse than none, so this gate fails when:

  1. an IPC enum listed under `[[ipc]]` does not exist at the named file,
     has a variant the inventory does not list, or the inventory lists a
     variant the enum does not have (both directions, exact);
  2. a search-plane or Native control opcode enum exists without an `[[ipc]]`
     row, including a tagged enum directly carried by another Native opcode;
  3. a format-version constant in a workspace `src/` tree is not named by
     exactly one `[[artifact]]`, or is named with a value other than the
     one the code declares (a bump without an inventory update fails);
  4. an inventory constant does not exist in the code at the named file.
  5. a tools-owned JSON artifact has no owner/producer/consumer/schema mapping,
     or its registered version disagrees with the schema's version constant.

Legacy flat search-plane enums and one-line format constants use the existing
strict regexes. Native control variants use the Rust AST.
"""

from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
INVENTORY_PATH = ROOT / "tools" / "ci" / "inventory" / "wire-surface.toml"

# The only files that may declare the search-plane IPC opcode enums.
SEARCH_PLANE_IPC_FILES: tuple[str, ...] = (
    "crates/quanta-index-contract/src/ipc/split.rs",
    "crates/quanta-index-contract/src/ipc/ingest/envelope.rs",
)
NATIVE_IPC_FILE = "crates/quanta-index-contract/src/native_protocol.rs"
NATIVE_IPC_ROOTS = {"NativeIndexRequest": "request", "NativeIndexResponse": "response"}

IPC_ENUM_NAME_RE = re.compile(
    r"^SearchPlane(?P<plane>Query|Control|Ingest)Ipc(?P<direction>Request|Response)$"
)
NATIVE_UNIT_BLOCK_RE = re.compile(rb"^\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*\{\s*units\s*\{([^{}]*)\}")
NATIVE_UNIT_TAG_RE = re.compile(rb"\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*[0-9]+\s*")
NATIVE_CFG_TEST_RE = re.compile(rb"#\[\s*cfg\s*\(\s*test\s*\)\s*\]")
ENUM_DECL_RE = re.compile(r"^\s*pub\s+enum\s+(?P<name>[A-Z][A-Za-z0-9_]*)\s*\{\s*$")
VARIANT_RE = re.compile(r"^\s*(?P<name>[A-Z][A-Za-z0-9_]*)\s*\(")

# One-line integer constant declarations; only the names whose suffix is in
# FORMAT_CONST_SUFFIXES are on-disk format versions. Anything else named
# `*VERSION` (library pins, DSL tags, schema-column names) is not one.
INT_CONST_RE = re.compile(
    r"^\s*(?:pub(?:\([a-z]+\))?\s+)?const\s+(?P<name>[A-Z][A-Z0-9_]*)"
    r"\s*:\s*u(?:8|16|32|64)\s*=\s*(?P<value>[0-9][0-9_]*)\s*;"
)
FORMAT_CONST_SUFFIXES: tuple[str, ...] = (
    "FORMAT_VERSION",
    "_FORMAT",
    "CONTRACT_VERSION",
    "TEXT_AUTHORITY_VERSION",
)
FORMAT_SCHEMA_SUFFIX_RE = re.compile(r"SCHEMA_V[0-9]+$")


def is_format_constant_name(name: str) -> bool:
    return name.endswith(FORMAT_CONST_SUFFIXES) or bool(FORMAT_SCHEMA_SUFFIX_RE.search(name))


# The text normalizer stamp is a struct constant with a major/minor pair.
NORMALIZER_CONST_RE = re.compile(
    r"^\s*(?:pub(?:\([a-z]+\))?\s+)?const\s+(?P<name>TEXT_NORMALIZER_VERSION)\s*:\s*"
    r"TextNormalizerVersion\s*=\s*$"
)
NORMALIZER_VALUE_RE = re.compile(
    r"^\s*TextNormalizerVersion\s*\{\s*major:\s*(?P<major>[0-9]+)\s*,\s*minor:\s*(?P<minor>[0-9]+)\s*\}\s*;"
)
CFG_TEST_RE = re.compile(r"^\s*#\[cfg\(test\)\]\s*$")

REPRODUCTION_CLASSES: frozenset[str] = frozenset(
    {
        "producer-rebuild",
        "current-format-backup",
        "reject-only",
        "cache",
        "vendor-native",
    }
)

TOOL_MIGRATION_FIXTURES = {
    "deployment_selection_unsupported_version_refused": (
        "tools/ci/tests/test_deployment_launcher.py",
        "test_unsupported_selection_version_is_refused",
    ),
    "proof_manifest_v0_refused": (
        "tools/ci/tests/test_check_proof_authority.py",
        "test_manifest_v0_is_refused",
    ),
    "proof_aggregate_v1_refused": (
        "tools/ci/tests/test_write_proof_aggregate.py",
        "test_ready_aggregate_is_final_release_receipt",
    ),
    "error_authority_inventory_source_digest_changes": (
        "tools/ci/tests/test_write_error_authority_inventory.py",
        "test_inventory_source_digest_changes_with_raw_source_bytes",
    ),
    "search_plane_error_table_stale_content_refused": (
        "tools/ci/tests/test_check_search_plane_error_codes.py",
        "test_committed_table_check_rejects_stale_content",
    ),
    "verification_receipt_v1_not_release_authority": (
        "tools/ci/tests/test_check_proof_authority.py",
        "test_legacy_verification_receipt_is_not_proof_manifest",
    ),
    "verification_receipt_v1_refused_by_retrieval": (
        "tools/ci/tests/test_retrieval_receipt_version_contract.py",
        "test_retrieval_refuses_legacy_verification_receipt",
    ),
}


def check_tool_artifacts(inventory: dict, root: Path = ROOT) -> list[Finding]:
    """Validate tools-owned JSON formats that do not have Rust constants."""
    findings: list[Finding] = []
    ids: set[str] = set()
    schema_versions: dict[str, set[int]] = {}
    claimed_versions: dict[str, set[int]] = {}
    for row in inventory.get("tool_artifact", []):
        artifact_id = row.get("id")
        where = f"wire-surface.toml [[tool_artifact]] id={artifact_id!r}"
        if not isinstance(artifact_id, str) or not artifact_id:
            findings.append(Finding(where, "`id` must be a non-empty string"))
            continue
        if artifact_id in ids:
            findings.append(Finding(where, "listed twice"))
            continue
        ids.add(artifact_id)
        for field in (
            "owner",
            "path",
            "producer",
            "schema_file",
            "decoder",
            "compatibility",
            "migration_fixture",
            "notes",
        ):
            if not isinstance(row.get(field), str) or not row[field]:
                findings.append(Finding(where, f"`{field}` must be a non-empty string"))
        consumers = row.get("consumers")
        if (
            not isinstance(consumers, list)
            or not consumers
            or any(not isinstance(consumer, str) or not consumer for consumer in consumers)
        ):
            findings.append(Finding(where, "`consumers` must be a non-empty string array"))
        version = row.get("version")
        if not isinstance(version, int) or isinstance(version, bool) or version < 1:
            findings.append(Finding(where, "`version` must be a positive integer"))
        migration_fixture = row.get("migration_fixture")
        if migration_fixture not in TOOL_MIGRATION_FIXTURES:
            findings.append(
                Finding(
                    where,
                    f"`migration_fixture` is not an executable registered fixture ID: {migration_fixture!r}",
                )
            )
        else:
            fixture_file, fixture_function = TOOL_MIGRATION_FIXTURES[migration_fixture]
            fixture_path = root / fixture_file
            try:
                fixture_source = fixture_path.read_text(encoding="utf-8")
            except OSError as error:
                findings.append(Finding(where, f"cannot read fixture file {fixture_file}: {error}"))
            else:
                if (
                    re.search(rf"^def {re.escape(fixture_function)}\s*\(", fixture_source, re.M)
                    is None
                ):
                    findings.append(
                        Finding(
                            where,
                            f"registered fixture function is missing: {fixture_file}::{fixture_function}",
                        )
                    )
        owner = row.get("owner")
        if isinstance(owner, str) and not (root / owner).is_file():
            findings.append(Finding(where, f"owner file does not exist: {owner}"))
        decoder = row.get("decoder")
        if isinstance(decoder, str) and not (root / decoder).is_file():
            findings.append(Finding(where, f"decoder file does not exist: {decoder}"))
        schema_file = row.get("schema_file")
        if isinstance(schema_file, str):
            schema_path = root / schema_file
            if not schema_path.is_file():
                findings.append(Finding(where, f"schema file does not exist: {schema_file}"))
            else:
                try:
                    schema = json.loads(schema_path.read_text(encoding="utf-8"))
                    version_property = schema["properties"]["schema_version"]
                    if "const" in version_property:
                        declared = version_property["const"]
                        if type(declared) is not int or declared < 1:
                            raise ValueError("schema_version const must be a positive integer")
                        supported = {declared}
                    else:
                        declared = None
                        values = version_property["enum"]
                        branches = schema["oneOf"]
                        if (
                            not isinstance(values, list)
                            or not values
                            or any(type(value) is not int or value < 1 for value in values)
                            or len(set(values)) != len(values)
                            or not isinstance(branches, list)
                        ):
                            raise ValueError("invalid schema_version enum or oneOf")
                        branch_versions = [
                            branch["properties"]["schema_version"]["const"] for branch in branches
                        ]
                        if (
                            len(branch_versions) != len(values)
                            or any(type(value) is not int for value in branch_versions)
                            or set(branch_versions) != set(values)
                        ):
                            raise ValueError(
                                "oneOf version branches must match schema_version enum"
                            )
                        supported = set(values)
                except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
                    findings.append(
                        Finding(where, f"schema has no readable schema_version contract: {error}")
                    )
                else:
                    schema_versions[schema_file] = supported
                    if type(version) is not int or version < 1:
                        continue
                    if version not in supported:
                        findings.append(
                            Finding(
                                where,
                                f"inventory version {version!r} differs from schema const {declared!r}"
                                if declared is not None
                                else f"inventory version {version!r} is not supported by schema {sorted(supported)!r}",
                            )
                        )
                    elif version in claimed_versions.setdefault(schema_file, set()):
                        findings.append(Finding(where, f"schema version {version} is listed twice"))
                    else:
                        claimed_versions[schema_file].add(version)
    for schema_file, supported in schema_versions.items():
        for missing in sorted(supported - claimed_versions.get(schema_file, set())):
            findings.append(Finding(schema_file, f"schema version {missing} has no inventory row"))
    return findings


@dataclass(frozen=True)
class Finding:
    where: str
    message: str

    def render(self) -> str:
        return f"{self.where}: {self.message}"


@dataclass(frozen=True)
class CodeConstant:
    file: str
    name: str
    value: str


def workspace_members(root: Path = ROOT) -> list[Path]:
    data = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    return [root / member for member in data.get("workspace", {}).get("members", [])]


def crate_source_files(root: Path = ROOT) -> list[Path]:
    files: list[Path] = []
    for member in workspace_members(root):
        src = member / "src"
        if not src.is_dir():
            continue
        for rs in sorted(src.rglob("*.rs")):
            if "tests" in rs.relative_to(src).parts:
                continue
            files.append(rs)
    return files


def parse_ipc_enums(text: str) -> dict[str, list[str]]:
    """Every `pub enum SearchPlane*Ipc{Request,Response}` and its variants.

    Only flat newtype variants (`Name(Payload),`) are recognized; that is the
    shape of every opcode enum, and a variant written any other way is a
    finding rather than a silently skipped line.
    """
    enums: dict[str, list[str]] = {}
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        decl = ENUM_DECL_RE.match(lines[index])
        if not decl or not IPC_ENUM_NAME_RE.match(decl.group("name")):
            index += 1
            continue
        name = decl.group("name")
        variants: list[str] = []
        index += 1
        while index < len(lines):
            line = lines[index]
            stripped = line.strip()
            if stripped == "}":
                break
            if not stripped or stripped.startswith("//") or stripped.startswith("#["):
                index += 1
                continue
            variant = VARIANT_RE.match(line)
            if not variant:
                raise ValueError(f"{name}: unrecognized variant line `{stripped}`")
            variants.append(variant.group("name"))
            index += 1
        enums[name] = variants
        index += 1
    return enums


def native_unit_tags(scope, enum_name: str) -> set[str]:
    tags: set[str] | None = None
    for item in scope.named_children:
        macro = item.named_children[0] if item.type == "expression_statement" else item
        if macro.type != "macro_invocation":
            continue
        macro_name = macro.child_by_field_name("macro")
        if macro_name is None or macro_name.text not in {
            b"native_serde_enum",
            b"quanta_index_contract_base::native_serde_enum",
        }:
            continue
        token_tree = macro.named_children[-1]
        block = NATIVE_UNIT_BLOCK_RE.match(token_tree.text)
        if block is None or block[1].decode("ascii") != enum_name:
            continue
        if tags is not None:
            raise ValueError(f"{enum_name}: duplicate native wire declarations")
        tags = set()
        for declaration in block[2].strip().rstrip(b",").split(b",") if block[2].strip() else ():
            unit = NATIVE_UNIT_TAG_RE.fullmatch(declaration)
            if unit is None:
                raise ValueError(f"{enum_name}: malformed native unit tag")
            variant = unit[1].decode("ascii")
            if variant in tags:
                raise ValueError(f"{enum_name}: native unit `{variant}` tagged twice")
            tags.add(variant)
    return tags or set()


def native_enum_variants(item, name: str, tagged_units: set[str]) -> tuple[list[str], set[str]]:
    visibility = next((node for node in item.children if node.type == "visibility_modifier"), None)
    if visibility is None or visibility.text != b"pub":
        raise ValueError(f"{name}: control enum must be public")
    body = item.child_by_field_name("body")
    if body is None:
        raise ValueError(f"{name}: enum body is absent")
    variants: list[str] = []
    payloads: set[str] = set()
    seen_units: set[str] = set()
    for child in body.named_children:
        if child.type != "enum_variant":
            continue
        variant_node = child.child_by_field_name("name")
        if variant_node is None:
            raise ValueError(f"{name}: unrecognized native variant shape")
        variant = variant_node.text.decode("utf-8").removeprefix("r#")
        fields = child.child_by_field_name("body")
        if fields is None:
            if child.child_count != 1 or variant not in tagged_units:
                raise ValueError(f"{name}: unrecognized native variant shape")
            seen_units.add(variant)
        elif fields.type == "ordered_field_declaration_list":
            types = [
                node
                for node in fields.named_children
                if node.type not in {"line_comment", "block_comment"}
            ]
            if len(types) == 1:
                payload = types[0]
                if payload.type in {"type_identifier", "scoped_type_identifier"}:
                    payloads.add(payload.text.decode("utf-8"))
        elif fields.type != "field_declaration_list":
            raise ValueError(f"{name}: unrecognized native variant shape")
        if variant in variants:
            raise ValueError(f"{name}: variant `{variant}` declared twice")
        variants.append(variant)
    if seen_units != tagged_units:
        raise ValueError(f"{name}: native unit tags differ from enum variants")
    return variants, payloads


@dataclass(frozen=True)
class NativeOpcode:
    file: str
    direction: str
    variants: list[str]


def parse_native_enums(sources: dict[str, str]) -> dict[str, NativeOpcode]:
    """Follow Native envelope variants through directly carried tagged enums."""
    try:
        from tree_sitter_language_pack import get_parser
    except ImportError as error:
        raise ValueError("native wire discovery requires tree-sitter-language-pack") from error

    parser = get_parser("rust")
    definitions: dict[str, tuple[str, object, object, tuple[str, ...]]] = {}
    by_name: dict[str, set[str]] = {}
    imports: dict[tuple[str, ...], dict[str, set[str]]] = {}
    native_dir = Path(NATIVE_IPC_FILE).with_suffix("")

    def use_targets(node, prefix: str = "") -> list[tuple[str, str]]:
        if node.type == "use_as_clause":
            path = node.child_by_field_name("path")
            alias = node.child_by_field_name("alias")
            if path is not None and alias is not None:
                target = (
                    f"{prefix}::{path.text.decode('utf-8')}"
                    if prefix
                    else path.text.decode("utf-8")
                )
                return [(alias.text.decode("utf-8"), target)]
        if node.type == "scoped_use_list":
            path = node.child_by_field_name("path")
            group = next((child for child in node.named_children if child.type == "use_list"), None)
            if path is not None and group is not None:
                target_prefix = (
                    f"{prefix}::{path.text.decode('utf-8')}"
                    if prefix
                    else path.text.decode("utf-8")
                )
                return use_targets(group, target_prefix)
        if node.type in {"use_declaration", "use_list"}:
            return [
                target for child in node.named_children for target in use_targets(child, prefix)
            ]
        if node.type in {"identifier", "scoped_identifier"}:
            path = node.text.decode("utf-8")
            target = f"{prefix}::{path}" if prefix else path
            return [(path.rsplit("::", 1)[-1], target)]
        return []

    def collect(scope, rel: str, module_path: tuple[str, ...]) -> None:
        test_only = False
        for item in scope.named_children:
            if item.type == "attribute_item":
                test_only |= bool(NATIVE_CFG_TEST_RE.fullmatch(item.text))
                continue
            if item.type in {"line_comment", "block_comment"}:
                continue
            if test_only:
                test_only = False
                continue
            if item.type == "use_declaration":
                local = imports.setdefault(module_path, {})
                for alias, target in use_targets(item):
                    local.setdefault(alias, set()).add(target)
            elif item.type == "type_item":
                name = item.child_by_field_name("name")
                target = item.child_by_field_name("type")
                if (
                    name is not None
                    and target is not None
                    and target.type
                    in {
                        "type_identifier",
                        "scoped_type_identifier",
                    }
                ):
                    imports.setdefault(module_path, {}).setdefault(
                        name.text.decode("utf-8"), set()
                    ).add(target.text.decode("utf-8"))
            elif (
                item.type == "enum_item" and (node := item.child_by_field_name("name")) is not None
            ):
                name = node.text.decode("utf-8").removeprefix("r#")
                qualified = "::".join((*module_path, name))
                if qualified in definitions:
                    raise ValueError(f"{qualified}: native enum declared twice")
                definitions[qualified] = (rel, item, scope, module_path)
                by_name.setdefault(name, set()).add(qualified)
            elif item.type == "mod_item":
                name = item.child_by_field_name("name")
                body = item.child_by_field_name("body")
                if name is not None and body is not None:
                    collect(body, rel, (*module_path, name.text.decode("utf-8")))

    for rel, text in sources.items():
        tree = parser.parse(text.encode("utf-8"))
        if tree.root_node.has_error:
            raise ValueError(f"{rel}: native control Rust syntax could not be parsed")
        if rel == NATIVE_IPC_FILE:
            module_path = ("crate", "native_protocol")
        else:
            member = Path(rel).relative_to(native_dir)
            parts = member.parts[:-1] + (() if member.stem == "mod" else (member.stem,))
            module_path = ("crate", "native_protocol", *parts)
        collect(tree.root_node, rel, module_path)

    def resolve_direct_path(path: str, module_path: tuple[str, ...]) -> str | None:
        parts = tuple(part.removeprefix("r#") for part in path.split("::"))
        if parts[0] == "crate":
            qualified = "::".join(parts)
        elif parts[0] == "self":
            qualified = "::".join((*module_path, *parts[1:]))
        elif parts[0] == "super":
            parent = module_path
            while parts and parts[0] == "super":
                parent = parent[:-1]
                parts = parts[1:]
            qualified = "::".join((*parent, *parts))
        else:
            qualified = "::".join((*module_path, *parts))
        if qualified in definitions:
            return qualified
        return None

    def resolve_payload(payload: str, module_path: tuple[str, ...]) -> str | None:
        direct = resolve_direct_path(payload, module_path)
        if direct is not None:
            return direct
        parts = tuple(part.removeprefix("r#") for part in payload.split("::"))
        if len(parts) == 1:
            targets = imports.get(module_path, {}).get(parts[0])
            if targets is not None:
                if len(targets) != 1:
                    raise ValueError(f"{payload}: ambiguous native opcode import")
                return resolve_direct_path(next(iter(targets)), module_path)
            matches = by_name.get(parts[0], set())
            if len(matches) > 1:
                raise ValueError(f"{payload}: ambiguous unqualified native opcode payload")
            return next(iter(matches), None)
        return None

    enums: dict[str, NativeOpcode] = {}
    visited: dict[str, str] = {}
    pending = [
        (f"crate::native_protocol::{name}", direction)
        for name, direction in NATIVE_IPC_ROOTS.items()
    ]
    while pending:
        qualified, direction = pending.pop()
        name = qualified.rsplit("::", 1)[-1]
        if (known := visited.get(qualified)) is not None:
            if known != direction:
                raise ValueError(f"{name}: native opcode reached as both request and response")
            continue
        if qualified not in definitions:
            raise ValueError(f"{name}: expected one native enum declaration, found 0")
        if name in enums:
            raise ValueError(f"{name}: multiple native opcodes require distinct inventory names")
        rel, item, scope, module_path = definitions[qualified]
        variants, payloads = native_enum_variants(item, name, native_unit_tags(scope, name))
        visited[qualified] = direction
        enums[name] = NativeOpcode(rel, direction, variants)
        pending.extend(
            (resolved, direction)
            for payload in sorted(payloads)
            if (resolved := resolve_payload(payload, module_path)) is not None
        )
    return enums


def parse_format_constants(rel: str, text: str) -> list[CodeConstant]:
    """Every on-disk format-version constant declared in one source file.

    `rel` is the file's path relative to the workspace root, as the
    inventory names it. `#[cfg(test)]` constants are fixture names, not
    formats, and are skipped.
    """
    found: list[CodeConstant] = []
    lines = text.splitlines()
    for index, line in enumerate(lines):
        previous = lines[index - 1] if index > 0 else ""
        if CFG_TEST_RE.match(previous):
            continue
        match = INT_CONST_RE.match(line)
        if match and is_format_constant_name(match.group("name")):
            found.append(
                CodeConstant(rel, match.group("name"), match.group("value").replace("_", ""))
            )
            continue
        normalizer = NORMALIZER_CONST_RE.match(line)
        if normalizer and index + 1 < len(lines):
            value = NORMALIZER_VALUE_RE.match(lines[index + 1])
            if not value:
                raise ValueError(
                    f"{rel}:{index + 2}: TEXT_NORMALIZER_VERSION value line is not "
                    "`TextNormalizerVersion { major: N, minor: M };`"
                )
            found.append(
                CodeConstant(
                    rel,
                    normalizer.group("name"),
                    f"{value.group('major')}.{value.group('minor')}",
                )
            )
    return found


def load_inventory(path: Path = INVENTORY_PATH) -> dict:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def check_ipc(inventory: dict, root: Path = ROOT) -> list[Finding]:
    findings: list[Finding] = []
    code_enums: dict[str, tuple[str, list[str]]] = {}
    for rel in SEARCH_PLANE_IPC_FILES:
        path = root / rel
        if not path.is_file():
            findings.append(Finding(rel, "IPC file listed in the checker does not exist"))
            continue
        try:
            parsed = parse_ipc_enums(path.read_text(encoding="utf-8"))
        except ValueError as err:
            findings.append(Finding(rel, str(err)))
            continue
        for name, variants in parsed.items():
            if name in code_enums:
                findings.append(Finding(rel, f"`{name}` is declared in more than one IPC file"))
                continue
            code_enums[name] = (rel, variants)

    native_file = root / NATIVE_IPC_FILE
    native_dir = native_file.with_suffix("")
    try:
        sources = {
            path.relative_to(root).as_posix(): path.read_text(encoding="utf-8")
            for path in (native_file, *sorted(native_dir.rglob("*.rs")))
        }
        native_enums = parse_native_enums(sources)
    except (OSError, ValueError) as error:
        findings.append(Finding(NATIVE_IPC_FILE, str(error)))
        native_enums = {}
    code_enums.update((name, (enum.file, enum.variants)) for name, enum in native_enums.items())

    listed: set[str] = set()
    for row in inventory.get("ipc", []):
        name = row.get("enum")
        where = f"wire-surface.toml [[ipc]] enum={name!r}"
        search_plane = IPC_ENUM_NAME_RE.match(name) if isinstance(name, str) else None
        if not isinstance(name, str) or (search_plane is None and name not in native_enums):
            findings.append(
                Finding(where, "enum is not a known search-plane or native control name")
            )
            continue
        if name in listed:
            findings.append(Finding(where, "listed twice"))
            continue
        listed.add(name)
        if name not in code_enums:
            findings.append(Finding(where, "no such enum in the IPC files"))
            continue
        code_file, code_variants = code_enums[name]
        if row.get("file") != code_file:
            findings.append(
                Finding(where, f"file is {row.get('file')!r}; the enum lives in {code_file!r}")
            )
        inventory_variants = row.get("variants")
        if not isinstance(inventory_variants, list) or not all(
            isinstance(v, str) for v in inventory_variants
        ):
            findings.append(Finding(where, "`variants` must be a list of strings"))
            continue
        if len(set(inventory_variants)) != len(inventory_variants):
            findings.append(Finding(where, "`variants` lists a name twice"))
        for missing in [v for v in code_variants if v not in inventory_variants]:
            findings.append(
                Finding(where, f"variant `{missing}` exists in the code but is not listed")
            )
        for extra in [v for v in inventory_variants if v not in code_variants]:
            findings.append(
                Finding(where, f"variant `{extra}` is listed but the enum has no such variant")
            )
        if search_plane is None:
            label, expected_plane = "native control", "control"
            expected_direction = native_enums[name].direction
        else:
            label = "search-plane"
            expected_plane = search_plane["plane"].lower()
            expected_direction = search_plane["direction"].lower()
        if row.get("plane") != expected_plane or row.get("direction") != expected_direction:
            findings.append(
                Finding(
                    where,
                    f"{label} row must use plane={expected_plane!r} and "
                    f"direction={expected_direction!r}",
                )
            )

    for name, (code_file, _variants) in sorted(code_enums.items()):
        if name not in listed:
            findings.append(
                Finding(f"{code_file}", f"`{name}` has no [[ipc]] row in the inventory")
            )
    return findings


def check_artifacts(inventory: dict, root: Path = ROOT) -> list[Finding]:
    findings: list[Finding] = []
    code_constants: dict[tuple[str, str], str] = {}
    for path in crate_source_files(root):
        rel = path.relative_to(root).as_posix()
        try:
            constants = parse_format_constants(rel, path.read_text(encoding="utf-8"))
        except ValueError as err:
            findings.append(Finding(rel, str(err)))
            continue
        for constant in constants:
            key = (constant.file, constant.name)
            if key in code_constants:
                findings.append(
                    Finding(constant.file, f"`{constant.name}` is declared twice in one file")
                )
                continue
            code_constants[key] = constant.value

    claimed: dict[tuple[str, str], str] = {}
    ids: set[str] = set()
    for row in inventory.get("artifact", []):
        artifact_id = row.get("id")
        where = f"wire-surface.toml [[artifact]] id={artifact_id!r}"
        if not isinstance(artifact_id, str) or not artifact_id:
            findings.append(Finding(where, "`id` must be a non-empty string"))
            continue
        if artifact_id in ids:
            findings.append(Finding(where, "listed twice"))
            continue
        ids.add(artifact_id)
        for field in ("owner", "path", "notes"):
            if not isinstance(row.get(field), str) or not row.get(field):
                findings.append(Finding(where, f"`{field}` must be a non-empty string"))
        if row.get("reproduction") not in REPRODUCTION_CLASSES:
            findings.append(
                Finding(
                    where,
                    f"`reproduction` must be one of {sorted(REPRODUCTION_CLASSES)}",
                )
            )
        owner = row.get("owner")
        if isinstance(owner, str) and not (root / "crates" / owner / "Cargo.toml").is_file():
            findings.append(Finding(where, f"owner crate `{owner}` does not exist"))
        for constant in row.get("version_constants", []):
            file = constant.get("file")
            name = constant.get("name")
            value = constant.get("value")
            cwhere = f"{where} constant={name!r}"
            if not isinstance(file, str) or not isinstance(name, str):
                findings.append(Finding(cwhere, "`file` and `name` must be strings"))
                continue
            key = (file, name)
            if key in claimed:
                findings.append(Finding(cwhere, f"already claimed by artifact {claimed[key]!r}"))
                continue
            claimed[key] = artifact_id
            if key not in code_constants:
                findings.append(Finding(cwhere, f"no format-version constant `{name}` in `{file}`"))
                continue
            if str(value) != code_constants[key]:
                findings.append(
                    Finding(
                        cwhere,
                        f"inventory says {value!r}; the code declares {code_constants[key]!r}",
                    )
                )

    for (file, name), value in sorted(code_constants.items()):
        if (file, name) not in claimed:
            findings.append(
                Finding(
                    file,
                    f"format-version constant `{name}` = {value} is not named by any [[artifact]]",
                )
            )
    return findings


def check(inventory: dict, root: Path = ROOT) -> list[Finding]:
    if inventory.get("schema") != 2:
        return [Finding("wire-surface.toml", "`schema` must be 2")]
    return (
        check_ipc(inventory, root)
        + check_artifacts(inventory, root)
        + check_tool_artifacts(inventory, root)
    )


def main() -> int:
    if not INVENTORY_PATH.is_file():
        print(f"missing inventory: {INVENTORY_PATH}", file=sys.stderr)
        return 1
    findings = check(load_inventory())
    if findings:
        print("wire-surface inventory drifted from the code:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding.render()}", file=sys.stderr)
        print(
            "\nUpdate tools/ci/inventory/wire-surface.toml in the same commit as the "
            "opcode or format change.",
            file=sys.stderr,
        )
        return 1
    print("wire-surface inventory matches the code.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
