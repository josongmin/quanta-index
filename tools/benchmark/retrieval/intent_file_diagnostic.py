"""Gold-blind, typed file ranking diagnostic over paired real SDK chunk lanes.

This does not alter ordinary Hybrid ranking or the official ARB scorer. The
input record must be one bound SDK execution of both lexical and semantic
routes. The policy uses file ranks, never raw lane scores or Gin outcomes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath
from typing import Any

POLICY_ID = "intent-file-rrf-v1"
RRF_K = 60
LANE_TOP_K = 100
MAX_ADDED_IMPORTS = 8

_RUST_USE = re.compile(
    r"^\+\s*(?:pub(?:\([^)]*\))?\s+)?use\s+crate::([A-Za-z_][A-Za-z_0-9]*(?:::[A-Za-z_][A-Za-z_0-9]*)+)\s*;\s*$"
)
_DIFF_HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(?:[^\n]*)$")


class DiagnosticRefusal(ValueError):
    """The supplied SDK record or declared intent is not bound to this policy."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise DiagnosticRefusal(message)


def _test_file(path: str) -> bool:
    parts = PurePosixPath(path).parts
    name = parts[-1].lower()
    return (
        "tests" in parts
        or "__tests__" in parts
        or name.startswith("test_")
        or name.endswith(
            ("_test.go", "_test.rs", "_test.py", ".test.ts", ".test.tsx", ".spec.ts", ".spec.tsx")
        )
    )


def _file_lane(route: dict[str, Any], name: str) -> dict[tuple[str, str], dict[str, Any]]:
    _require(isinstance(route, dict), f"{name}: missing SDK route")
    rows = route.get("candidates")
    _require(isinstance(rows, list) and len(rows) <= LANE_TOP_K, f"{name}: invalid lane rows")
    window = route.get("window")
    _require(
        isinstance(window, dict) and window.get("returned") == len(rows),
        f"{name}: invalid SDK window",
    )
    files: dict[tuple[str, str], dict[str, Any]] = {}
    seen_ids: dict[str, tuple[str, str]] = {}
    for chunk_rank, row in enumerate(rows, 1):
        _require(isinstance(row, dict), f"{name}: invalid candidate")
        candidate_id = row.get("candidate_id")
        path = row.get("path")
        repo = row.get("source_repo_id")
        _require(
            all(isinstance(value, str) and value for value in (candidate_id, path, repo))
            and not PurePosixPath(path).is_absolute()
            and ".." not in PurePosixPath(path).parts,
            f"{name}: invalid candidate identity",
        )
        key = (repo, path)
        _require(seen_ids.setdefault(candidate_id, key) == key, f"{name}: reused candidate ID")
        if key not in files:
            files[key] = {
                "file_rank": len(files) + 1,
                "chunk_rank": chunk_rank,
                "candidate_id": candidate_id,
            }
    return files


def rank_bound_sdk_record(
    record: dict[str, Any], pack_task: dict[str, Any], public_sample: dict[str, Any]
) -> dict[str, Any]:
    """Rank files from one SDK record under declared code2test/edit2ripple intent.

    `public_sample` contains only id, task_type and query from the original
    sample. The caller must not pass gold or derive intent from result paths.
    """
    _require(set(public_sample) == {"id", "task_type", "query"}, "sample is not blind-projected")
    sample_id = public_sample["id"]
    task_type = public_sample["task_type"]
    query = public_sample["query"]
    _require(
        isinstance(sample_id, str)
        and bool(sample_id)
        and task_type in ("code2test", "edit2ripple")
        and isinstance(query, dict),
        "unsupported intent",
    )
    if task_type == "edit2ripple":
        anchor = query.get("anchor_file")
        _require(isinstance(anchor, str) and bool(anchor), "edit2ripple requires anchor_file")
    else:
        anchor = None
    task_id = f"ARB-{sample_id}"
    adapted_text = pack_task.get("query")
    _require(
        pack_task.get("task_id") == task_id and isinstance(adapted_text, str), "pack task mismatch"
    )
    query_sha = hashlib.sha256(adapted_text.encode("utf-8")).hexdigest()
    _require(pack_task.get("query_sha256") == query_sha, "pack query digest mismatch")
    tasks = record.get("tasks")
    _require(isinstance(tasks, list) and len(tasks) == 1, "record must have one task")
    task = tasks[0]
    _require(isinstance(task, dict) and task.get("task_id") == task_id, "SDK task mismatch")
    identity = task.get("query_identity")
    _require(isinstance(identity, dict), "missing SDK query identity")
    _require(
        identity.get("original_query_sha256") == query_sha
        and identity.get("semantic_text_sha256") == query_sha,
        "SDK query identity mismatch",
    )
    lexical = _file_lane(task.get("lexical"), "lexical")
    semantic = _file_lane(task.get("semantic"), "semantic")
    # A stable candidate ID cannot name different files across routes.
    ids = {}
    for route_name in ("lexical", "semantic"):
        for row in task[route_name]["candidates"]:
            key = (row["source_repo_id"], row["path"])
            _require(
                ids.setdefault(row["candidate_id"], key) == key,
                "cross-route candidate ID collision",
            )
    results = []
    for repo, path in lexical.keys() | semantic.keys():
        key = (repo, path)
        lex = lexical.get(key)
        sem = semantic.get(key)
        score = sum(1.0 / (RRF_K + witness["file_rank"]) for witness in (lex, sem) if witness)
        _require(math.isfinite(score), "invalid file score")
        results.append(
            {
                "source_repo_id": repo,
                "path": path,
                "file_rrf_score": score,
                "lexical": lex,
                "semantic": sem,
                "is_test_file": _test_file(path),
                "is_anchor_file": path == anchor if anchor is not None else False,
            }
        )
    if task_type == "code2test":
        results.sort(
            key=lambda row: (
                not row["is_test_file"],
                -row["file_rrf_score"],
                row["lexical"] is None,
                row["source_repo_id"],
                row["path"],
            )
        )
    else:
        results.sort(
            key=lambda row: (
                row["is_anchor_file"],
                -row["file_rrf_score"],
                row["lexical"] is None,
                row["source_repo_id"],
                row["path"],
            )
        )
    return {
        "policy": POLICY_ID,
        "task_id": task_id,
        "task_type": task_type,
        "bound_query_sha256": query_sha,
        "results": results[:20],
        "file_universe_size": len(results),
        "lane_top_k": LANE_TOP_K,
    }


def _apply_unified_diff(source: str, diff: str) -> tuple[str, set[int]]:
    """Apply one exact hunk stream to a pinned file and return added line numbers."""
    _require(isinstance(diff, str) and len(diff) <= 1_000_000, "invalid anchor diff")
    old = source.splitlines(keepends=True)
    lines = diff.splitlines(keepends=True)
    # The benchmark transports the last hunk line without its terminal LF.
    if lines and not lines[-1].endswith("\n") and source.endswith("\n"):
        lines[-1] += "\n"
    output: list[str] = []
    added: set[int] = set()
    source_offset = 0
    position = 0
    _require(bool(lines), "anchor diff has no hunks")
    while position < len(lines):
        header = _DIFF_HUNK.fullmatch(lines[position].rstrip("\n"))
        _require(header is not None, "invalid anchor diff hunk")
        old_start = int(header[1])
        old_count = int(header[2]) if header[2] is not None else 1
        new_start = int(header[3])
        new_count = int(header[4]) if header[4] is not None else 1
        target = old_start - 1 if old_count else old_start
        _require(source_offset <= target <= len(old), "anchor diff old range differs")
        output.extend(old[source_offset:target])
        source_offset = target
        _require(
            len(output) + 1 == new_start or new_count == 0 and len(output) == new_start,
            "anchor diff new range differs",
        )
        position += 1
        consumed = produced = 0
        while position < len(lines) and not lines[position].startswith("@@"):
            line = lines[position]
            _require(line and line[0] in (" ", "+", "-"), "invalid anchor diff line")
            body = line[1:]
            if line[0] in (" ", "-"):
                _require(
                    source_offset < len(old) and old[source_offset] == body,
                    "anchor diff context differs from pinned source",
                )
                source_offset += 1
                consumed += 1
            if line[0] in (" ", "+"):
                output.append(body)
                produced += 1
                if line[0] == "+":
                    added.add(len(output))
            position += 1
        _require(consumed == old_count and produced == new_count, "anchor diff hunk width differs")
    output.extend(old[source_offset:])
    return "".join(output), added


def _complete_diff_prefix(anchor: str, diff: str) -> tuple[str, str]:
    _require(isinstance(diff, str) and len(diff) <= 1_000_000, "invalid anchor diff")
    lines = diff.splitlines(keepends=True)
    if not lines or lines[-1].rstrip("\n") != "...[truncated]":
        return diff, "complete"
    # ARB marks a clipped last hunk. A Rust import may be admitted from an
    # earlier complete hunk. A Go field proof needs the full reconstructed file.
    _require(anchor.endswith(".rs"), "Go anchor diff is truncated")
    headers = [index for index, line in enumerate(lines) if _DIFF_HUNK.fullmatch(line.rstrip("\n"))]
    _require(len(headers) >= 2, "anchor diff has no complete hunk before truncation")
    return "".join(lines[: headers[-1]]), "complete_hunk_prefix"


def _go_syntax(
    anchor: str, patched: str, added: set[int], sources: dict[str, str]
) -> dict[str, Any]:
    """Ask Go's parser for syntax nodes, with every file already SHA-pinned."""
    helper = Path(__file__).with_name("go_edit_syntax.go")
    request = json.dumps(
        {"anchor": anchor, "patched": patched, "added_lines": sorted(added), "sources": sources}
    )
    environment = {
        **os.environ,
        "GO111MODULE": "off",
        "GOENV": "off",
        "GOFLAGS": "",
        "GOPROXY": "off",
        "GOSUMDB": "off",
        "GOTOOLCHAIN": "local",
        "GOWORK": "off",
    }
    try:
        result = subprocess.run(
            ["go", "run", str(helper)],
            input=request,
            text=True,
            capture_output=True,
            timeout=60,
            check=False,
            env=environment,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise DiagnosticRefusal(f"Go syntax parser unavailable: {error}") from error
    _require(result.returncode == 0, f"Go syntax parser refused: {result.stderr.strip()}")
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise DiagnosticRefusal("Go syntax parser returned invalid JSON") from error
    _require(
        isinstance(value, dict) and set(value) == {"imports", "selectors", "structs"},
        "Go syntax parser returned invalid shape",
    )
    return value


def _go_package_sources(anchor: str, root: Path, files: dict[str, str]) -> dict[str, str]:
    directory = str(PurePosixPath(anchor).parent)
    prefix = "" if directory == "." else directory + "/"
    paths = [
        path
        for path in files
        if path.startswith(prefix)
        and "/" not in path[len(prefix) :]
        and path.endswith(".go")
        and not path.endswith("_test.go")
    ]
    _require(anchor in paths and len(paths) <= 256, "Go package source bound differs")
    return {path: _verified_source(root, path, files).decode("utf-8") for path in sorted(paths)}


def _rust_added_imports(patched: str, added: set[int]) -> list[tuple[str, str]]:
    """Recognize simple top-level use declarations outside strings and macros."""
    imports = []
    block_depth = 0
    raw_end: str | None = None
    quoted = False
    escaped = False
    delimiters: list[str] = []
    closing = {"}": "{", ")": "(", "]": "["}
    for line_number, line in enumerate(patched.splitlines(), 1):
        masked = list(line)
        index = 0
        while index < len(line):
            if raw_end is not None:
                end = line.find(raw_end, index)
                if end < 0:
                    masked[index:] = " " * (len(line) - index)
                    break
                masked[index : end + len(raw_end)] = " " * (end + len(raw_end) - index)
                index = end + len(raw_end)
                raw_end = None
                continue
            if block_depth:
                opening = line.startswith("/*", index)
                closing = line.startswith("*/", index)
                if opening:
                    block_depth += 1
                elif closing:
                    block_depth -= 1
                width = 2 if opening or closing else 1
                masked[index : index + width] = " " * width
                index += width
                continue
            if quoted:
                if escaped:
                    escaped = False
                elif line[index] == "\\":
                    escaped = True
                elif line[index] == '"':
                    quoted = False
                masked[index] = " "
                index += 1
                continue
            if line.startswith("//", index):
                masked[index:] = " " * (len(line) - index)
                break
            if line.startswith("/*", index):
                block_depth = 1
                masked[index : index + 2] = "  "
                index += 2
                continue
            raw = re.match(r'(?:[bc]?r)(#{0,255})"', line[index:])
            if raw:
                raw_end = '"' + raw.group(1)
                width = len(raw.group())
                masked[index : index + width] = " " * width
                index += width
                continue
            if line[index] == '"':
                quoted = True
                masked[index] = " "
            elif line[index] == "'":
                char = re.match(
                    r"'(?:[^'\\]|\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.))'", line[index:]
                )
                if char:
                    width = len(char.group())
                    masked[index : index + width] = " " * width
                    index += width
                    continue
            index += 1
        code = "".join(masked)
        if line_number in added and not delimiters:
            match = _RUST_USE.match("+" + code)
            if match:
                imports.append(("rust", match.group(1)))
        for token in code:
            if token in "{([":
                delimiters.append(token)
            elif token in closing:
                _require(
                    delimiters and delimiters.pop() == closing[token],
                    "patched Rust source has mismatched delimiters",
                )
    _require(
        block_depth == 0 and raw_end is None and not quoted and not delimiters,
        "patched Rust source has unterminated token or delimiter",
    )
    _require(len(imports) <= MAX_ADDED_IMPORTS, "too many added imports")
    return list(dict.fromkeys(imports))


def _manifest_files(manifest: dict[str, Any], pack: dict[str, Any]) -> dict[str, str]:
    _require(isinstance(manifest, dict) and isinstance(pack, dict), "invalid source binding")
    _require(
        isinstance(manifest.get("repository_commit"), str)
        and pack.get("repository_commit") == manifest["repository_commit"],
        "source revision mismatch",
    )
    files = manifest.get("files")
    universe = pack.get("file_universe")
    _require(isinstance(files, list) and isinstance(universe, list), "missing source universe")

    def rows(entries: list[Any]) -> dict[str, str]:
        result: dict[str, str] = {}
        for entry in entries:
            _require(isinstance(entry, dict), "invalid source entry")
            path, digest = entry.get("path"), entry.get("file_sha256")
            _require(
                isinstance(path, str)
                and bool(path)
                and not PurePosixPath(path).is_absolute()
                and ".." not in PurePosixPath(path).parts
                and isinstance(digest, str)
                and re.fullmatch(r"[0-9a-f]{64}", digest) is not None
                and path not in result,
                "invalid source identity",
            )
            result[path] = digest
        return result

    bound = rows(files)
    _require(bound == rows(universe), "SDK/source file universe mismatch")
    return bound


def _verified_source(root: Path, path: str, files: dict[str, str]) -> bytes:
    _require(path in files, f"source path is not pinned: {path}")
    _require(
        isinstance(root, Path) and root.is_dir() and not root.is_symlink(), "invalid source root"
    )
    source = root.joinpath(*PurePosixPath(path).parts)
    _require(
        source.is_file()
        and not source.is_symlink()
        and source.resolve().is_relative_to(root.resolve()),
        f"missing pinned source: {path}",
    )
    raw = source.read_bytes()
    _require(hashlib.sha256(raw).hexdigest() == files[path], f"source digest mismatch: {path}")
    return raw


def _rust_destination(anchor: str, import_path: str, files: dict[str, str]) -> str | None:
    parts = PurePosixPath(anchor).parts
    _require(
        parts.count("src") == 1 and anchor.endswith(".rs"), "Rust anchor has no unique src root"
    )
    root = PurePosixPath(*parts[: parts.index("src") + 1])
    segments = import_path.split("::")
    _require(len(segments) >= 2, "Rust import has no module destination")
    candidates = []
    for module in (segments[:-1], segments):
        path = root.joinpath(*module)
        for candidate in (f"{path}.rs", f"{path}/mod.rs"):
            if candidate in files:
                candidates.append(candidate)
    candidates = list(dict.fromkeys(candidates))
    _require(len(candidates) <= 1, f"ambiguous Rust import destination: {import_path}")
    return candidates[0] if candidates else None


def _go_destination(import_path: str, root: Path, files: dict[str, str]) -> str | None:
    go_mod = _verified_source(root, "go.mod", files).decode("utf-8")
    modules = re.findall(r"(?m)^module\s+(\S+)\s*$", go_mod)
    _require(len(modules) == 1, "Go module declaration is ambiguous")
    module = modules[0]
    if import_path == module:
        directory = ""
    elif import_path.startswith(module + "/"):
        directory = import_path[len(module) + 1 :]
    else:
        return None
    _require(
        directory == ""
        or (not directory.startswith("/") and ".." not in PurePosixPath(directory).parts),
        "unsafe Go import path",
    )
    prefix = directory + "/" if directory else ""
    candidates = [
        path
        for path in files
        if path.startswith(prefix)
        and "/" not in path[len(prefix) :]
        and path.endswith(".go")
        and not path.endswith("_test.go")
    ]
    _require(len(candidates) <= 1, f"ambiguous Go package destination: {import_path}")
    return candidates[0] if candidates else None


def _bound_edit_source(
    record: dict[str, Any],
    pack: dict[str, Any],
    public_sample: dict[str, Any],
    manifest: dict[str, Any],
    source_root: Path,
    original_query_sha256: str,
) -> tuple[str, dict[str, Any], str, dict[str, str], str, str, set[int], str]:
    """Bind a raw edit query, SDK lanes and exact source before typed analysis."""
    _require(public_sample.get("task_type") == "edit2ripple", "wrong typed intent")
    _require(isinstance(public_sample.get("id"), str), "missing sample id")
    query = public_sample.get("query")
    _require(isinstance(query, dict), "missing public query")
    original = hashlib.sha256(
        json.dumps(query, ensure_ascii=False, sort_keys=True).encode()
    ).hexdigest()
    _require(original == original_query_sha256, "original query digest mismatch")
    task_id = "ARB-" + public_sample.get("id", "")
    pack_tasks = pack.get("tasks")
    _require(isinstance(pack_tasks, list), "missing pack tasks")
    matching = [
        task for task in pack_tasks if isinstance(task, dict) and task.get("task_id") == task_id
    ]
    _require(len(matching) == 1, "pack task identity mismatch")
    # Rebuild the frozen adapter's text from the public input before accepting
    # any import in raw text that the 32-term SDK query may have dropped.
    from tools.benchmark.retrieval.arb_adapter import adapt

    adapted = adapt(public_sample)
    _require(adapted["adapted_text"] == matching[0].get("query"), "SDK adapter input mismatch")
    rank_bound_sdk_record(record, matching[0], public_sample)
    files = _manifest_files(manifest, pack)
    generation = record.get("generation")
    _require(
        isinstance(generation, dict)
        and generation.get("revision_id") == manifest["repository_commit"],
        "SDK generation/source mismatch",
    )
    anchor = query.get("anchor_file")
    _require(isinstance(anchor, str), "missing anchor path")
    source = _verified_source(source_root, anchor, files).decode("utf-8")
    diff, diff_coverage = _complete_diff_prefix(anchor, query.get("anchor_diff"))
    patched, added_lines = _apply_unified_diff(source, diff)
    return task_id, query, anchor, files, original, patched, added_lines, diff_coverage


def resolve_edit2ripple_import_destinations(
    record: dict[str, Any],
    pack: dict[str, Any],
    public_sample: dict[str, Any],
    manifest: dict[str, Any],
    source_root: Path,
    original_query_sha256: str,
) -> dict[str, Any]:
    """Separate exact import-destination lane over pinned pre-edit source.

    A destination is admitted only when an added import and the source tree
    identify one exact file. No gold, score weight or generic Hybrid order is
    consulted. Ambiguous package/module paths refuse instead of guessing.
    """
    task_id, _query, anchor, files, original, patched, added_lines, diff_coverage = (
        _bound_edit_source(
            record, pack, public_sample, manifest, source_root, original_query_sha256
        )
    )
    if anchor.endswith(".go"):
        syntax = _go_syntax(
            anchor, patched, added_lines, _go_package_sources(anchor, source_root, files)
        )
        imports = [("go", row["path"]) for row in syntax["imports"]]
    elif anchor.endswith(".rs"):
        imports = _rust_added_imports(patched, added_lines)
    else:
        raise DiagnosticRefusal("import lane requires Go or Rust anchor")
    _require(len(imports) <= MAX_ADDED_IMPORTS, "too many added imports")
    imports = list(dict.fromkeys(imports))
    destinations = []
    unresolved = []
    for kind, import_path in imports:
        if kind == "rust":
            path = _rust_destination(anchor, import_path, files)
        else:
            path = _go_destination(import_path, source_root, files)
        if path is None:
            unresolved.append({"kind": kind, "import": import_path})
            continue
        _verified_source(source_root, path, files)
        destinations.append(
            {
                "path": path,
                "file_sha256": files[path],
                "kind": kind,
                "added_import": import_path,
                "anchor_file": anchor,
            }
        )
    distinct = {row["path"]: row for row in destinations}
    return {
        "policy": "edit2ripple-added-import-destination-v1",
        "task_id": task_id,
        "status": "no_added_import"
        if not imports
        else "partial"
        if distinct and unresolved
        else "resolved"
        if distinct
        else "unresolved",
        "candidates": [distinct[path] for path in sorted(distinct)],
        "unresolved_imports": unresolved,
        "source_revision": manifest["repository_commit"],
        "original_query_sha256": original,
        "diff_coverage": diff_coverage,
    }


def resolve_edit2ripple_go_field_owner(
    record: dict[str, Any],
    pack: dict[str, Any],
    public_sample: dict[str, Any],
    manifest: dict[str, Any],
    source_root: Path,
    original_query_sha256: str,
) -> dict[str, Any]:
    """Separate exact owner of a pre-existing Go field in an added selector.

    A new field name or header literal provides no pre-edit declaration and
    therefore cannot enter this lane. This intentionally does not infer a
    target from naming similarity or change the file RRF result.
    """
    task_id, _query, anchor, files, _original, patched, added_lines, _coverage = _bound_edit_source(
        record, pack, public_sample, manifest, source_root, original_query_sha256
    )
    _require(anchor.endswith(".go"), "field-owner lane requires Go anchor")
    syntax = _go_syntax(
        anchor, patched, added_lines, _go_package_sources(anchor, source_root, files)
    )
    selectors = {
        (
            row["receiver_type"],
            row["root_name"],
            row["receiver_field"],
            row["target_field"],
        )
        for row in syntax["selectors"]
    }
    _require(len(selectors) <= MAX_ADDED_IMPORTS, "too many added Go selectors")
    if not selectors:
        return {
            "policy": "edit2ripple-go-field-owner-v1",
            "task_id": task_id,
            "status": "no_exact_added_selector",
            "candidates": [],
            "unresolved_selectors": [],
        }
    structs = syntax["structs"]
    destinations = []
    unresolved = []
    for receiver_type, root_name, receiver_field, target_field in sorted(selectors):
        receiver_declarations = [
            row for row in structs if row["name"] == receiver_type and row["path"] == anchor
        ]
        _require(
            len(receiver_declarations) == 1,
            f"Go receiver struct absent or ambiguous: {receiver_type}",
        )
        field_types = [
            field["type"]
            for field in receiver_declarations[0]["fields"]
            if field["name"] == receiver_field
        ]
        if not field_types:
            unresolved.append(f"{root_name}.{receiver_field}.{target_field}")
            continue
        _require(
            len(field_types) == 1 and bool(field_types[0]),
            f"ambiguous receiver field: {receiver_field}",
        )
        owner_type = field_types[0]
        declarations = [row for row in structs if row["name"] == owner_type]
        _require(len(declarations) <= 1, f"ambiguous Go field owner: {owner_type}.{target_field}")
        owners = [
            row["path"]
            for row in declarations
            for field in row["fields"]
            if field["name"] == target_field
        ]
        _require(len(owners) <= 1, f"ambiguous Go field owner: {owner_type}.{target_field}")
        if not owners:
            unresolved.append(f"{root_name}.{receiver_field}.{target_field}")
            continue
        destinations.append(
            {
                "path": owners[0],
                "file_sha256": files[owners[0]],
                "selector": f"{root_name}.{receiver_field}.{target_field}",
                "declaration": f"{owner_type}.{target_field}",
                "anchor_file": anchor,
            }
        )
    distinct = {row["path"]: row for row in destinations}
    return {
        "policy": "edit2ripple-go-field-owner-v1",
        "task_id": task_id,
        "status": "no_exact_added_selector"
        if not selectors
        else "partial"
        if distinct and unresolved
        else "resolved"
        if distinct
        else "unresolved",
        "candidates": [distinct[path] for path in sorted(distinct)],
        "unresolved_selectors": unresolved,
    }


def _unique_json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        _require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _read_json(path: Path, expected_sha256: str | None = None) -> Any:
    _require(
        path.is_absolute() and path.is_file() and not path.is_symlink(),
        f"invalid input path: {path}",
    )
    raw = path.read_bytes()
    if expected_sha256 is not None:
        _require(
            hashlib.sha256(raw).hexdigest() == expected_sha256, f"input digest mismatch: {path}"
        )
    try:
        return json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=_unique_json_object,
            parse_constant=lambda value: (_ for _ in ()).throw(
                DiagnosticRefusal(f"invalid JSON value: {value}")
            ),
        )
    except (UnicodeError, json.JSONDecodeError) as error:
        raise DiagnosticRefusal(f"invalid JSON input: {path}: {error}") from error


def run_opt_in_candidate_lanes(
    record_path: Path,
    pack_path: Path,
    sample_path: Path,
    manifest_path: Path,
    source_root: Path,
    binding_path: Path,
    lane: str,
) -> dict[str, Any]:
    """Run only typed candidate evidence over externally pinned benchmark inputs."""
    binding = _read_json(binding_path)
    _require(
        isinstance(binding, dict)
        and set(binding)
        == {
            "schema_version",
            "task_id",
            "original_query_sha256",
            "record_sha256",
            "pack_sha256",
            "public_sample_sha256",
            "manifest_sha256",
            "source_root",
            "repository_commit",
        },
        "invalid opt-in binding",
    )
    _require(
        binding["schema_version"] == 1 and lane in ("imports", "go_field_owner", "both"),
        "invalid opt-in lane or binding version",
    )
    for key in (
        "original_query_sha256",
        "record_sha256",
        "pack_sha256",
        "public_sample_sha256",
        "manifest_sha256",
    ):
        _require(
            isinstance(binding[key], str)
            and re.fullmatch(r"[0-9a-f]{64}", binding[key]) is not None,
            f"invalid bound digest: {key}",
        )
    _require(
        source_root.is_absolute() and str(source_root) == binding["source_root"],
        "source root differs from binding",
    )
    record = _read_json(record_path, binding["record_sha256"])
    pack = _read_json(pack_path, binding["pack_sha256"])
    public_sample = _read_json(sample_path, binding["public_sample_sha256"])
    manifest = _read_json(manifest_path, binding["manifest_sha256"])
    _require(
        isinstance(public_sample, dict) and set(public_sample) == {"id", "task_type", "query"},
        "sample is not blind-projected",
    )
    _require(
        isinstance(public_sample["id"], str) and binding["task_id"] == "ARB-" + public_sample["id"],
        "bound task mismatch",
    )
    _require(
        isinstance(public_sample["query"], dict)
        and not set(public_sample["query"]).intersection({"gold", "gold_files", "qrels", "labels"}),
        "public query contains gold-bearing key",
    )
    _require(
        isinstance(pack, dict)
        and pack.get("schema_version") == 3
        and pack.get("repository_commit") == binding["repository_commit"],
        "bound pack mismatch",
    )
    _require(
        isinstance(manifest, dict)
        and manifest.get("repository_commit") == binding["repository_commit"],
        "bound manifest mismatch",
    )
    _require(
        isinstance(record, dict) and isinstance(record.get("tasks"), list), "invalid SDK record"
    )
    matching = [
        task
        for task in record["tasks"]
        if isinstance(task, dict) and task.get("task_id") == binding["task_id"]
    ]
    _require(len(matching) == 1, "bound SDK task missing or ambiguous")
    one_task_record = {"generation": record.get("generation"), "tasks": matching}
    args = (
        one_task_record,
        pack,
        public_sample,
        manifest,
        source_root,
        binding["original_query_sha256"],
    )
    lanes = {}
    if lane in ("imports", "both"):
        lanes["imports"] = resolve_edit2ripple_import_destinations(*args)
    if lane in ("go_field_owner", "both"):
        lanes["go_field_owner"] = resolve_edit2ripple_go_field_owner(*args)
    return {
        "schema_version": 1,
        "status": "typed_candidate_diagnostic_only",
        "task_id": binding["task_id"],
        "repository_commit": binding["repository_commit"],
        "bound_record_sha256": binding["record_sha256"],
        "bound_original_query_sha256": binding["original_query_sha256"],
        "lanes": lanes,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Bound, gold-blind edit2ripple candidate diagnostic. Go anchors require a local Go toolchain."
    )
    for name in ("record", "pack", "public-sample", "manifest", "source-root", "binding"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--lane", choices=("imports", "go_field_owner", "both"), default="imports")
    args = parser.parse_args(argv)
    try:
        result = run_opt_in_candidate_lanes(
            args.record,
            args.pack,
            args.public_sample,
            args.manifest,
            args.source_root,
            args.binding,
            args.lane,
        )
    except DiagnosticRefusal as error:
        print(f"diagnostic refusal: {error}", file=sys.stderr)
        return 2
    print(json.dumps(result, ensure_ascii=False, sort_keys=True, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
