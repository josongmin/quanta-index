#!/usr/bin/env python3
"""Generate or verify the closed search-plane error-code table."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tempfile
from pathlib import Path

import jsonschema

ROOT = Path(__file__).resolve().parents[2]
ENUM_SOURCE = Path("crates/quanta-index-contract/src/ipc/error.rs")
LEXICAL_SOURCE = Path("crates/quanta-index-contract/src/lex/error_code.rs")
CONTRACT_BASE_ROOT = Path("crates/quanta-index-contract-base/src")
SCHEMA = Path("tools/ci/search-plane-error-code-table.schema.json")
TABLE = Path("tools/ci/inventory/search-plane-error-codes.json")
DOMAIN = b"quanta-index/search-plane-error-enum-source/v2"


class TableError(ValueError):
    """The source or committed table is not the exact closed authority."""


def _one_match(pattern: str, text: str, *, label: str) -> str:
    matches = re.findall(pattern, text, flags=re.DOTALL)
    if len(matches) != 1:
        raise TableError(f"expected exactly one {label}, found {len(matches)}")
    return matches[0]


def _constant_strings(root: Path) -> dict[str, str]:
    constants: dict[str, str] = {}
    for path in sorted((root / CONTRACT_BASE_ROOT).rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        for name, wire in re.findall(
            r'pub const ([A-Z][A-Z0-9_]*):\s*&str\s*=\s*"([A-Z][A-Z0-9_]*)"\s*;',
            text,
        ):
            prior = constants.setdefault(name, wire)
            if prior != wire:
                raise TableError(f"constant {name} has conflicting values")
    return constants


def source_codes(root: Path) -> list[str]:
    error_text = (root / ENUM_SOURCE).read_text(encoding="utf-8")
    invocation = _one_match(
        r"define_search_plane_error_codes!\s*\{(.*?)\n\}",
        error_text,
        label="define_search_plane_error_codes invocation",
    )
    lexical_body = _one_match(
        r"lexical\s*\[(.*?)\]\s*;",
        invocation,
        label="lexical variant list",
    )
    native_body = _one_match(
        r"native\s*\[(.*?)\]\s*;",
        invocation,
        label="native variant list",
    )
    lexical_variants = re.findall(r"\b([A-Z][A-Za-z0-9]*)\b", lexical_body)
    native_entries = re.findall(r'\b([A-Z][A-Za-z0-9]*)\s*=>\s*"([A-Z][A-Z0-9_]*)"', native_body)
    if not lexical_variants or not native_entries:
        raise TableError("closed enum invocation has an empty lexical or native set")

    lexical_text = (root / LEXICAL_SOURCE).read_text(encoding="utf-8")
    lexical_all_body = _one_match(
        r"pub const ALL:\s*&'static \[Self\]\s*=\s*&\[(.*?)\]\s*;",
        lexical_text,
        label="LexicalErrorCode::ALL",
    )
    lexical_all = re.findall(r"Self::([A-Z][A-Za-z0-9]*)", lexical_all_body)
    if lexical_all != lexical_variants:
        raise TableError("nested lexical variant list differs from LexicalErrorCode::ALL")
    match_body = _one_match(
        r"pub const fn as_code_str\(self\).*?match self \{(.*?)\n\s*\}\n\s*\}",
        lexical_text,
        label="LexicalErrorCode::as_code_str match",
    )
    constants = _constant_strings(root)
    lexical_wires: dict[str, str] = {}
    for variant, literal, constant in re.findall(
        r'Self::([A-Z][A-Za-z0-9]*)\s*=>\s*(?:"([A-Z][A-Z0-9_]*)"|crate::([A-Z][A-Z0-9_]*))',
        match_body,
    ):
        wire = literal
        if constant:
            try:
                wire = constants[constant]
            except KeyError as error:
                raise TableError(f"unresolved lexical wire constant {constant}") from error
        lexical_wires[variant] = wire

    missing = sorted(set(lexical_variants) - set(lexical_wires))
    extra = sorted(set(lexical_wires) - set(lexical_variants))
    if missing or extra:
        raise TableError(f"lexical list/mapping differs: missing={missing} extra={extra}")

    codes = [lexical_wires[variant] for variant in lexical_variants]
    codes.extend(wire for _variant, wire in native_entries)
    if len(codes) != len(set(codes)):
        duplicates = sorted({code for code in codes if codes.count(code) > 1})
        raise TableError(f"wire codes are not unique: {duplicates}")
    return sorted(codes)


def enum_source_digest(root: Path) -> str:
    relative = ENUM_SOURCE.as_posix().encode()
    content = (root / ENUM_SOURCE).read_bytes()
    digest = hashlib.sha256()
    digest.update(len(DOMAIN).to_bytes(4, "big"))
    digest.update(DOMAIN)
    digest.update(len(relative).to_bytes(4, "big"))
    digest.update(relative)
    digest.update(len(content).to_bytes(8, "big"))
    digest.update(content)
    return f"sha256:{digest.hexdigest()}"


def expected_table(root: Path) -> dict[str, object]:
    codes = source_codes(root)
    return {
        "schema_version": 2,
        "enum_type": "SearchPlaneErrorCodeV2",
        "enum_source_path": ENUM_SOURCE.as_posix(),
        "enum_source_digest": enum_source_digest(root),
        "cardinality": len(codes),
        "codes": codes,
    }


def _validate_schema(root: Path, value: object) -> None:
    schema = json.loads((root / SCHEMA).read_text(encoding="utf-8"))
    jsonschema.Draft202012Validator(schema).validate(value)


def write_table(root: Path) -> None:
    value = expected_table(root)
    _validate_schema(root, value)
    output = root / TABLE
    output.parent.mkdir(parents=True, exist_ok=True)
    serialized = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    with tempfile.NamedTemporaryFile(dir=output.parent, delete=False) as handle:
        handle.write(serialized)
        temporary = Path(handle.name)
    temporary.replace(output)


def check_table(root: Path) -> None:
    expected = expected_table(root)
    actual = json.loads((root / TABLE).read_text(encoding="utf-8"))
    _validate_schema(root, actual)
    if actual != expected:
        raise TableError("committed search-plane error-code table differs from enum authority")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve()
    try:
        if args.write:
            write_table(root)
        check_table(root)
    except (OSError, json.JSONDecodeError, jsonschema.ValidationError, TableError) as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        return 1
    value = expected_table(root)
    digest = hashlib.sha256((root / TABLE).read_bytes()).hexdigest()
    print(f"OK search-plane-error-codes cardinality={value['cardinality']} sha256:{digest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
