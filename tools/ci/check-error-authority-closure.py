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

ROOT = Path(__file__).resolve().parents[2]
TABLE_CHECKER = ROOT / "tools/ci/check-search-plane-error-codes.py"
ALLOWED_LOWER_DOMAIN_PARSERS = {
    "crates/quanta-index-catalog/src/auxiliary.rs": {"track_from_code"},
    "crates/quanta-index-contract/src/query/history_order.rs": {"from_code_str"},
    "crates/quanta-index-core/src/domains/auxiliary.rs": {"from_code_str"},
    "crates/quanta-index-core/src/domains/generation.rs": {"from_code_str"},
}
FORBIDDEN_LINE_PATTERNS = {
    "free-form code field": re.compile(r"\bcode\s*:\s*String\b"),
    "dynamic code producer": re.compile(r"\bcode\s*:\s*format!\s*\("),
    "code substring classification": re.compile(
        r"\b(?:code|error_code)(?:\.(?:as_str|as_wire_str)\(\))?\.contains\s*\("
    ),
    "code string comparison": re.compile(
        r'\b(?:code|error_code)\s*(?:==|!=)\s*"|"[A-Z][A-Z0-9_]*"\s*(?:==|!=)\s*\b(?:code|error_code)\b'
    ),
}


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def scan_line(relative: str, line_number: int, line: str) -> list[str]:
    findings = [
        f"{relative}:{line_number}: {label}"
        for label, pattern in FORBIDDEN_LINE_PATTERNS.items()
        if pattern.search(line)
    ]
    if re.search(r"\bcode\s*:\s*&str\b", line):
        match = re.search(r"\bfn\s+(\w+)\s*\(", line)
        function_name = match.group(1) if match else None
        if function_name not in ALLOWED_LOWER_DOMAIN_PARSERS.get(relative, set()):
            findings.append(f"{relative}:{line_number}: unowned code &str pass-through")
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
        for line_number, line in enumerate(source.splitlines(), start=1):
            findings.extend(scan_line(relative, line_number, line))
        if relative == "crates/quanta-index-contract/src/ipc/error.rs" and '"BAD_REQUEST"' in source:
            findings.append(f"{relative}: BAD_REQUEST is not an accepted V2 code")
    if findings:
        print("REFUSED: free-form error authority remains:\n" + "\n".join(findings), file=sys.stderr)
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
        "crates/quanta-index-sdk/src/error.rs": (
            "code: SearchPlaneErrorCodeV2",
        ),
    }
    for relative, fragments in required_fragments.items():
        text = (ROOT / relative).read_text(encoding="utf-8")
        missing = [fragment for fragment in fragments if fragment not in text]
        if missing:
            print(f"REFUSED: {relative} misses typed authority fragments {missing}", file=sys.stderr)
            return 1

    print(
        "OK closed error authority: exact enum/table, typed core/IPC/SDK, "
        "free-form producer/pass-through paths=0"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
