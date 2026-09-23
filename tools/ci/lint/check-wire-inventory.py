#!/usr/bin/env python3
"""Keep `tools/ci/inventory/wire-surface.toml` exact against the code.

The inventory is the W1 consumer / wire inventory plan §11 requires before
any state or wire cutover: every IPC opcode and every on-disk artifact
format, with its version constant and its reproduction class. An inventory
that drifts from the code is worse than none, so this gate fails when:

  1. an IPC enum listed under `[[ipc]]` does not exist at the named file,
     has a variant the inventory does not list, or the inventory lists a
     variant the enum does not have (both directions, exact);
  2. an IPC enum matching `SearchPlane*Ipc{Request,Response}` exists in the
     known IPC files but no `[[ipc]]` row names it;
  3. a format-version constant in a workspace `src/` tree is not named by
     exactly one `[[artifact]]`, or is named with a value other than the
     one the code declares (a bump without an inventory update fails);
  4. an inventory constant does not exist in the code at the named file.
  5. a tools-owned JSON artifact has no owner/producer/consumer/schema mapping,
     or its registered version disagrees with the schema's version constant.

The Rust is read with regexes over the known files rather than parsed:
the enums are flat newtype-variant lists and the constants are one-line
integer declarations, and exactness on that shape beats cleverness.
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
IPC_FILES: tuple[str, ...] = (
    "crates/quanta-index-contract/src/ipc/split.rs",
    "crates/quanta-index-contract/src/ipc/ingest.rs",
)

IPC_ENUM_NAME_RE = re.compile(r"^SearchPlane(?:Query|Control|Ingest)Ipc(?:Request|Response)$")
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
        "offline-importer",
        "migration-input-only",
        "cache",
        "vendor-native",
    }
)

TOOL_MIGRATION_FIXTURES = {
    "proof_manifest_v0_refused": (
        "tools/ci/tests/test_check_proof_authority.py",
        "test_manifest_v0_is_refused",
    ),
    "proof_aggregate_missing_or_not_ready_refused": (
        "tools/ci/tests/test_write_proof_aggregate.py",
        "test_p12_guard_refuses_registered_not_ready_aggregate",
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
                            branch["properties"]["schema_version"]["const"]
                            for branch in branches
                        ]
                        if (
                            len(branch_versions) != len(values)
                            or any(type(value) is not int for value in branch_versions)
                            or set(branch_versions) != set(values)
                        ):
                            raise ValueError("oneOf version branches must match schema_version enum")
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
                                if declared is not None else
                                f"inventory version {version!r} is not supported by schema {sorted(supported)!r}",
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
            if any(part == "tests" for part in rs.parts):
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
    for rel in IPC_FILES:
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

    listed: set[str] = set()
    for row in inventory.get("ipc", []):
        name = row.get("enum")
        where = f"wire-surface.toml [[ipc]] enum={name!r}"
        if not isinstance(name, str) or not IPC_ENUM_NAME_RE.match(name):
            findings.append(Finding(where, "enum is not a SearchPlane*Ipc{Request,Response} name"))
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
        if row.get("direction") not in {"request", "response"}:
            findings.append(Finding(where, "`direction` must be `request` or `response`"))
        if row.get("plane") not in {"query", "control", "ingest"}:
            findings.append(Finding(where, "`plane` must be `query`, `control` or `ingest`"))

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
