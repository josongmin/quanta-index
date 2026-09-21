#!/usr/bin/env python3
"""Write a deterministic source-bound inventory for the error-code migration."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import tempfile
from bisect import bisect_right
from dataclasses import dataclass
from pathlib import Path
from re import Pattern

import jsonschema

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_PATH = ROOT / "tools/ci/error-authority-inventory.schema.json"
DEFAULT_OUTPUT = ROOT / "artifacts/sep-21/p00/error-authority-inventory.json"


@dataclass(frozen=True)
class Category:
    id: str
    pattern: Pattern[str]
    requires_semantic_migration_review: bool
    required_literal: str


CATEGORIES = (
    Category(
        "core-error-typed-constructor",
        re.compile(r"\bCoreError::Typed\s*\{"),
        False,
        "CoreError::Typed",
    ),
    Category(
        "search-plane-ipc-error-reference",
        re.compile(r"\bSearchPlaneIpcError\b"),
        False,
        "SearchPlaneIpcError",
    ),
    Category("free-form-code-string-field", re.compile(r"\bcode\s*:\s*String\b"), True, "code"),
    Category("dynamic-code-format", re.compile(r"\bcode\s*:\s*format!\s*\("), True, "code"),
    Category(
        "code-str-parameter", re.compile(r"\bcode\s*:\s*&(?:'[_a-zA-Z0-9]+\s+)?str\b"), True, "code"
    ),
    Category(
        "code-substring-classification",
        re.compile(r"(?:\bcode\b|\berror_code\b)[^\n;]{0,80}\.contains\s*\("),
        True,
        "code",
    ),
    Category(
        "code-string-equality",
        re.compile(
            r"(?:\bcode\b|\berror_code\b)[^\n;]{0,80}(?:==|!=)\s*\"|\"[^\n\"]+\"\s*(?:==|!=)[^\n;]{0,80}(?:\bcode\b|\berror_code\b)"
        ),
        True,
        "code",
    ),
    Category("bad-request-wire-literal", re.compile(r"\bBAD_REQUEST\b"), False, "BAD_REQUEST"),
)

MANUAL_GATES = (
    "SearchPlaneErrorCodeV2::ALL wire values are unique and equal the committed accepted-code table",
    "from_wire_str and serde reject every unknown wire code including the stale BAD_REQUEST fixture",
    "generic CoreError to wire mapping is one exhaustive typed owner",
    "SDK preserves SearchPlaneErrorCodeV2 without string downgrade",
)


def _git_head(root: Path) -> str:
    return subprocess.run(
        ["git", "-C", str(root), "rev-parse", "--verify", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def _source_files(root: Path) -> list[Path]:
    return sorted(path for path in (root / "crates").glob("*/src/**/*.rs") if path.is_file())


def build_inventory(root: Path) -> dict:
    files = _source_files(root)
    source_digest = hashlib.sha256()
    domain = b"quanta-index/error-authority-source/v1"
    source_digest.update(len(domain).to_bytes(4, "big"))
    source_digest.update(domain)
    records: dict[str, list[dict[str, object]]] = {category.id: [] for category in CATEGORIES}
    for path in files:
        relative = path.relative_to(root).as_posix()
        content = path.read_bytes()
        relative_bytes = relative.encode()
        source_digest.update(len(relative_bytes).to_bytes(4, "big"))
        source_digest.update(relative_bytes)
        source_digest.update(len(content).to_bytes(8, "big"))
        source_digest.update(content)
        # Match Path.read_text's universal-newline behavior while hashing raw bytes.
        text = content.decode("utf-8").replace("\r\n", "\n").replace("\r", "\n")
        lines = text.splitlines(keepends=True)
        offsets: list[int] = []
        offset = 0
        for line in lines:
            offsets.append(offset)
            offset += len(line)
        for category in CATEGORIES:
            if category.required_literal not in text:
                continue
            for match in category.pattern.finditer(text):
                line_index = bisect_right(offsets, match.start()) - 1
                line = lines[line_index] if lines else ""
                line_start = offsets[line_index] if offsets else 0
                records[category.id].append(
                    {
                        "path": relative,
                        "line": line_index + 1,
                        "column": match.start() - line_start + 1,
                        "line_sha256": hashlib.sha256(line.encode()).hexdigest(),
                    }
                )
    categories = [
        {
            "id": category.id,
            "requires_semantic_migration_review": category.requires_semantic_migration_review,
            "count": len(records[category.id]),
            "occurrences": records[category.id],
        }
        for category in CATEGORIES
    ]
    return {
        "schema_version": 1,
        "scope": {"include": "crates/*/src/**/*.rs", "exclude": []},
        "source_head": _git_head(root),
        "source_digest": f"sha256:{source_digest.hexdigest()}",
        "categories": categories,
        "p01_completion_requirements": {
            "manual_gates": list(MANUAL_GATES),
        },
        # Regex hits are discovery evidence, never semantic closure authority.
        "closed": False,
    }


def publish_inventory(*, root: Path, output: Path, require_closed: bool) -> tuple[Path, str, bool]:
    root = root.resolve()
    output = output.resolve()
    try:
        output.relative_to(root)
    except ValueError as error:
        raise ValueError("output must remain inside the repository") from error
    payload = build_inventory(root)
    schema = json.loads((root / SCHEMA_PATH.relative_to(ROOT)).read_text(encoding="utf-8"))
    jsonschema.Draft202012Validator(schema).validate(payload)
    output.parent.mkdir(parents=True, exist_ok=True)
    serialized = (json.dumps(payload, sort_keys=True, indent=2) + "\n").encode()
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=output.parent,
            prefix=f".{output.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            handle.write(serialized)
            handle.flush()
            os.fsync(handle.fileno())
            temporary = Path(handle.name)
        os.replace(temporary, output)
        temporary = None
        directory_fd = os.open(output.parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    return output, digest, False


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--require-closed", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    output = args.output if args.output.is_absolute() else root / args.output
    try:
        path, digest, closed = publish_inventory(
            root=root,
            output=output,
            require_closed=args.require_closed,
        )
    except (OSError, ValueError, json.JSONDecodeError, jsonschema.ValidationError) as error:
        print(f"REFUSED: {error}")
        return 2
    state = "BASELINE"
    print(f"WROTE {path.relative_to(root)} sha256:{digest} state={state}")
    if args.require_closed:
        print(
            "REFUSED: ErrorAuthorityInventoryV1 is discovery evidence; "
            "P01A must land the executable semantic closure validator"
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
