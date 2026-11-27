#!/usr/bin/env python3
"""Check exact closed error owners and semantic migration tripwires.

The P00 broad regex inventory is discovery-only and is deliberately not read
here. Rust owner tests and the dedicated proof recipe establish behavior;
these source checks reject known routes back to free-form wire authority.
"""

from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path

from tree_sitter_language_pack import get_parser

ROOT = Path(__file__).resolve().parents[2]
TABLE_CHECKER = ROOT / "tools/ci/check-search-plane-error-codes.py"
TEST_CFG = re.compile(rb"#\s*!?\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
OTHER_ERROR_ENUMS = {
    "crates/quanta-index-contract/src/native_cutover_evidence_v1.rs": b"NativeCutoverControlTerminalV1",
    "crates/quanta-index-searchd-harness/src/open_loop_accounting.rs": b"Outcome",
}
ALLOWED_LOWER_DOMAIN_PARSERS = {
    "crates/quanta-index-catalog/src/auxiliary.rs": {"track_from_code"},
    "crates/quanta-index-contract/src/query/history_order.rs": {"from_code_str"},
    "crates/quanta-index-core/src/domains/auxiliary.rs": {"from_code_str"},
    "crates/quanta-index-core/src/domains/generation.rs": {"from_code_str"},
}
FORBIDDEN_PATTERNS = {
    "free-form code field": re.compile(rb"\bcode\s*:\s*String\b"),
    "dynamic code producer": re.compile(rb"\bcode\s*:\s*format\s*!\s*\("),
    "code substring classification": re.compile(
        rb"\b(?:code|error_code)\b(?:\s*\.\s*(?:as_str|as_wire_str)\s*\(\s*\))?\s*\.\s*contains\s*\("
    ),
    "code string comparison": re.compile(
        rb'\b(?:code|error_code)\b\s*(?:==|!=)\s*"'
        rb'|"(?:\s*|[A-Z][A-Z0-9_]*)"\s*(?:==|!=)\s*\b(?:code|error_code)\b'
    ),
}
CODE_IDENTIFIER = re.compile(rb"\b(?:code|error_code)\b")
CODE_REFERENCE = re.compile(rb"\bcode\s*:\s*&\s*str\b")


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _is_other_error_enum(node, owner: bytes | None) -> bool:
    if not owner or node.has_error:
        return False
    if node.type == "enum_item":
        name = node.child_by_field_name("name")
        return name is not None and name.text == owner
    if node.type == "macro_invocation":
        macro = node.child_by_field_name("macro")
        tokens = node.named_children[-1].named_children
        return (
            macro is not None
            and macro.text == b"crate::native_serde_enum"
            and bool(tokens)
            and tokens[0].text == owner
        )
    return False


def _outside_search_plane(node, owner: bytes | None):
    test_only = False
    for child in node.named_children:
        if child.type in {"line_comment", "block_comment"}:
            continue
        if child.type == "inner_attribute_item" and TEST_CFG.fullmatch(child.text):
            if not node.has_error:
                yield node
            return
        if child.type == "attribute_item":
            test_only |= TEST_CFG.fullmatch(child.text) is not None
            continue
        if (test_only and not child.has_error) or _is_other_error_enum(child, owner):
            yield child
        elif not child.is_error:
            yield from _outside_search_plane(child, owner)
        test_only = False


def _normalized_source(root, source: bytes, owner: bytes | None) -> bytes:
    excluded = {(node.start_byte, node.end_byte) for node in _outside_search_plane(root, owner)}
    retained = bytearray(source)
    pending = [root]
    while pending:
        node = pending.pop()
        bounds = (node.start_byte, node.end_byte)
        if bounds in excluded or node.type in {"line_comment", "block_comment"}:
            retained[node.start_byte : node.end_byte] = re.sub(rb"[^\r\n]", b" ", node.text)
        elif node.type in {"string_literal", "raw_string_literal"}:
            # Keep quote tokens for comparisons without scanning example code in strings.
            text = re.sub(rb"[^\r\n]", b" ", node.text)
            retained[node.start_byte : node.end_byte] = b'"' + text[1:-1] + b'"'
        else:
            pending.extend(node.named_children)
    return bytes(retained)


def _approved_code_reference(relative: str, root, match: re.Match[bytes]) -> bool:
    allowed = ALLOWED_LOWER_DOMAIN_PARSERS.get(relative)
    if not allowed:
        return False
    node = root.descendant_for_byte_range(match.start(), match.end())
    while node is not None and node.type not in {"parameters", "function_item"}:
        node = node.parent
    if node is None or node.type != "parameters" or node.has_error:
        return False
    if node.parent.type != "function_item":
        return False
    name = node.parent.child_by_field_name("name")
    return name is not None and name.text.decode("utf-8") in allowed


def scan_source(relative: str, source: str) -> list[str]:
    encoded = source.encode("utf-8")
    if not CODE_IDENTIFIER.search(encoded):
        return []
    root = get_parser("rust").parse(encoded).root_node
    # Keep recovered tokens visible while using intact syntax around them.
    retained = _normalized_source(root, encoded, OTHER_ERROR_ENUMS.get(relative))
    violations = [
        (match.start(), label)
        for label, pattern in FORBIDDEN_PATTERNS.items()
        for match in pattern.finditer(retained)
    ]
    violations.extend(
        (match.start(), "unowned code &str pass-through")
        for match in CODE_REFERENCE.finditer(retained)
        if not _approved_code_reference(relative, root, match)
    )
    findings = []
    for offset, reason in sorted(violations):
        line = encoded.count(b"\n", 0, offset) + 1
        findings.append(f"{relative}:{line}: {reason}")
    return findings


def main() -> int:
    table_module = _load("search_plane_error_codes", TABLE_CHECKER)
    try:
        table_module.check_table(ROOT)
    except Exception as error:  # exact checker owns the diagnostic type
        print(f"REFUSED: {error}", file=sys.stderr)
        return 1

    findings: list[str] = []
    for path in sorted((ROOT / "crates").glob("*/src/**/*.rs")):
        relative = path.relative_to(ROOT).as_posix()
        source = path.read_text(encoding="utf-8")
        findings.extend(scan_source(relative, source))
    if findings:
        print(
            "REFUSED: free-form error authority remains:\n" + "\n".join(findings), file=sys.stderr
        )
        return 1

    required_fragments = {
        "crates/quanta-index-contract/src/ipc/error.rs": (
            "pub enum SearchPlaneErrorCodeV2",
            "pub code: SearchPlaneErrorCodeV2",
            "pub const ALL: &'static [Self]",
            "pub fn from_wire_str(value: &str) -> Option<Self>",
        ),
        "crates/quanta-index-core/src/error.rs": (
            "code: SearchPlaneErrorCodeV2",
            "pub fn into_search_plane_wire(self)",
        ),
        "crates/quanta-index-sdk/src/error.rs": ("code: SearchPlaneErrorCodeV2",),
    }
    for relative, fragments in required_fragments.items():
        text = (ROOT / relative).read_text(encoding="utf-8")
        missing = [fragment for fragment in fragments if fragment not in text]
        if missing:
            print(
                f"REFUSED: {relative} misses typed authority fragments {missing}", file=sys.stderr
            )
            return 1

    print(
        "OK closed error authority: exact enum/table, typed core/IPC/SDK, "
        "free-form producer/pass-through paths=0"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
