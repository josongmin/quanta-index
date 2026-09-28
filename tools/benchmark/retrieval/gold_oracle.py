"""Independent, source-derived code-search labels for a narrow declared cohort.

This oracle never consumes product hits. Its definition cohort is specifically
named function declarations supported by the pinned Tree-sitter grammars, not
arbitrary semantic definitions. All generated labels remain unreviewed.
"""

from __future__ import annotations

import hashlib
import os
import re
from pathlib import Path

from tree_sitter_language_pack import get_parser

try:
    from evidence import IO_CHUNK_BYTES, EvidenceError, _consume_regular_file
except ModuleNotFoundError:  # package import outside the benchmark script path
    from tools.benchmark.evidence import IO_CHUNK_BYTES, EvidenceError, _consume_regular_file

ORACLE_VERSION = 1
MAX_TASKS = 2000
MAX_FILES = 4096
MAX_SOURCE_BYTES = 512 * 1024 * 1024
MAX_LABELS_PER_TASK = 20_000
MAX_TOTAL_LABELS = 100_000
LANGUAGE_SUFFIXES = {
    "python": frozenset({".py"}),
    "rust": frozenset({".rs"}),
    "go": frozenset({".go"}),
    "typescript": frozenset({".ts", ".tsx"}),
    "javascript": frozenset({".js", ".jsx", ".mjs", ".cjs"}),
}
FUNCTION_NODES = {
    "python": frozenset({"function_definition"}),
    "rust": frozenset({"function_item"}),
    "go": frozenset({"function_declaration", "method_declaration"}),
    "typescript": frozenset({"function_declaration", "method_definition"}),
    "javascript": frozenset({"function_declaration", "method_definition"}),
}
ID = re.compile(r"[A-Za-z][A-Za-z0-9_.-]*\Z")
BARE_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _bounded_source(path: Path, remaining: int) -> bytes:
    def consume(handle) -> bytes:
        if os.fstat(handle.fileno()).st_size > remaining:
            raise EvidenceError("gold source exceeds release byte limit")
        blocks, count = [], 0
        while block := handle.read(min(IO_CHUNK_BYTES, remaining - count + 1)):
            count += len(block)
            if count > remaining:
                raise EvidenceError("gold source exceeds release byte limit")
            blocks.append(block)
        return b"".join(blocks)

    return _consume_regular_file(path, consume)


def _path(path: object, *, allow_empty: bool = False) -> bool:
    if not isinstance(path, str):
        return False
    if not path:
        return allow_empty
    return (
        not path.startswith("/")
        and "\\" not in path
        and "\x00" not in path
        and all(
            part not in ("", ".", "..") and part.casefold() != ".git" for part in path.split("/")
        )
    )


def validate_recipe(recipe: object) -> dict:
    if (
        not isinstance(recipe, dict)
        or set(recipe) != {"schema_version", "tasks"}
        or type(recipe["schema_version"]) is not int
        or recipe["schema_version"] != 1
        or not isinstance(recipe["tasks"], list)
        or not 2 <= len(recipe["tasks"]) <= MAX_TASKS
    ):
        raise EvidenceError("gold recipe must have bounded schema v1 tasks")
    ids: set[str] = set()
    queries: set[tuple[str, str, str]] = set()
    normalized_queries: list[tuple[str, str, str]] = []
    families: dict[str, str] = {}
    splits: set[str] = set()
    for task in recipe["tasks"]:
        if not isinstance(task, dict) or set(task) != {
            "task_id",
            "split",
            "query_family_id",
            "intent",
            "query",
            "scope_prefix",
            "language",
            "case_semantics",
            "normalization",
        }:
            raise EvidenceError("gold recipe task has missing or unknown fields")
        task_id, split, family = task["task_id"], task["split"], task["query_family_id"]
        if (
            not isinstance(task_id, str)
            or not ID.fullmatch(task_id)
            or task_id in ids
            or split not in ("development", "holdout")
            or not isinstance(family, str)
            or not ID.fullmatch(family)
            or (family in families and families[family] != split)
        ):
            raise EvidenceError("duplicate task or cross-split query family")
        ids.add(task_id)
        families[family] = split
        splits.add(split)
        intent, query, language = task["intent"], task["query"], task["language"]
        if isinstance(query, str):
            try:
                query_bytes = query.encode("utf-8")
            except UnicodeError as error:
                raise EvidenceError("gold query is not valid UTF-8") from error
        else:
            query_bytes = b""
        if (
            intent not in ("literal_utf8_exact", "named_function_declaration")
            or not isinstance(query, str)
            or not query
            or len(query_bytes) > 1024
            or "\x00" in query
            or (intent == "named_function_declaration" and BARE_NAME.fullmatch(query) is None)
            or (
                intent == "named_function_declaration"
                and (not isinstance(language, str) or language not in LANGUAGE_SUFFIXES)
            )
            or (intent == "literal_utf8_exact" and language is not None)
            or not _path(task["scope_prefix"], allow_empty=True)
            or task["case_semantics"] != "sensitive"
            or task["normalization"] != "none_raw_utf8"
        ):
            raise EvidenceError("gold task has unsupported or ambiguous query semantics")
        query_key = (intent, query, task["scope_prefix"])
        if query_key in queries:
            raise EvidenceError("duplicate gold query/scope across tasks")
        queries.add(query_key)
        normalized = " ".join(query.casefold().split())
        normalized_queries.append((task_id, split, normalized))
    if splits != {"development", "holdout"}:
        raise EvidenceError("gold recipe requires development and holdout tasks")
    for index, (task_id, split, query) in enumerate(normalized_queries):
        for other_id, other_split, other_query in normalized_queries[index + 1 :]:
            if split == other_split:
                continue
            if query == other_query:
                raise EvidenceError(f"cross-split normalized query leakage: {task_id} / {other_id}")
            if min(len(query), len(other_query)) < 10:
                continue
            grams = {query[pos : pos + 5] for pos in range(len(query) - 4)}
            other_grams = {other_query[pos : pos + 5] for pos in range(len(other_query) - 4)}
            if len(grams & other_grams) / len(grams | other_grams) >= 0.8:
                raise EvidenceError(f"cross-split near-duplicate query: {task_id} / {other_id}")
    return recipe


def _literal_spans(raw: bytes, query: bytes) -> list[tuple[int, int, str]]:
    spans = []
    offset = 0
    while (start := raw.find(query, offset)) >= 0:
        spans.append((start, start + len(query), "literal_occurrence"))
        if len(spans) > MAX_LABELS_PER_TASK:
            raise EvidenceError("gold literal task exceeds label limit")
        offset = start + 1
    return spans


def _definition_spans(raw: bytes, query: bytes, language: str) -> tuple[list, str | None]:
    try:
        root = get_parser(language).parse(raw).root_node
    except (LookupError, ValueError) as error:
        return [], type(error).__name__
    if root.has_error:
        return [], "parser_error"
    spans = []
    nodes = [root]
    while nodes:
        node = nodes.pop()
        if node.type in FUNCTION_NODES[language]:
            name = node.child_by_field_name("name")
            if name is None:
                return [], "missing_declaration_name"
            if raw[name.start_byte : name.end_byte] == query:
                spans.append((name.start_byte, name.end_byte, node.type))
                if len(spans) > MAX_LABELS_PER_TASK:
                    raise EvidenceError("gold definition task exceeds label limit")
        nodes.extend(reversed(node.children))
    return sorted(spans), None


def derive(recipe: dict, manifest: dict, view: Path) -> tuple[dict, dict]:
    validate_recipe(recipe)
    if (
        not isinstance(manifest, dict)
        or set(manifest) != {"repository_commit", "files"}
        or not isinstance(manifest["files"], list)
        or not 0 < len(manifest["files"]) <= MAX_FILES
    ):
        raise EvidenceError("gold oracle requires a bounded release manifest")
    sources = {}
    total = 0
    for row in manifest["files"]:
        if (
            not isinstance(row, dict)
            or set(row) != {"path", "file_sha256"}
            or not _path(row["path"])
            or row["path"] in sources
            or not isinstance(row["file_sha256"], str)
        ):
            raise EvidenceError("gold release manifest has a duplicate or invalid file")
        raw = _bounded_source(view / row["path"], MAX_SOURCE_BYTES - total)
        total += len(raw)
        file_sha = _sha(raw)
        if total > MAX_SOURCE_BYTES or file_sha != row["file_sha256"]:
            raise EvidenceError("gold source exceeds limit or differs from release")
        sources[row["path"]] = (raw, file_sha)
    if list(sources) != sorted(sources):
        raise EvidenceError("gold release manifest files are not sorted")
    gold_tasks, blind_tasks = [], []
    label_count = 0
    paths_by_split: dict[str, set[str]] = {"development": set(), "holdout": set()}
    for task in recipe["tasks"]:
        query = task["query"].encode("utf-8")
        prefix = task["scope_prefix"]
        labels, unsupported = [], []
        selected = 0
        for path, (raw, file_sha) in sources.items():
            if prefix and not (path == prefix or path.startswith(prefix + "/")):
                continue
            if (
                task["intent"] == "named_function_declaration"
                and Path(path).suffix not in (LANGUAGE_SUFFIXES[task["language"]])
            ):
                continue
            selected += 1
            if task["intent"] == "literal_utf8_exact":
                spans = _literal_spans(raw, query)
                reason = None
            else:
                spans, reason = _definition_spans(raw, query, task["language"])
            if reason is not None:
                unsupported.append({"path": path, "reason": reason})
                continue
            for start, end, kind in spans:
                if len(labels) >= MAX_LABELS_PER_TASK:
                    raise EvidenceError("gold task exceeds label limit")
                label_count += 1
                if label_count > MAX_TOTAL_LABELS:
                    raise EvidenceError("gold capsule exceeds total label limit")
                labels.append(
                    {
                        "path": path,
                        "file_sha256": file_sha,
                        "start_byte": start,
                        "end_byte": end,
                        "kind": kind,
                        "local_name": task["query"]
                        if task["intent"] != "literal_utf8_exact"
                        else None,
                    }
                )
        if selected == 0:
            unsupported.append({"path": None, "reason": "empty_declared_scope"})
        labels.sort(key=lambda row: (row["path"], row["start_byte"], row["end_byte"]))
        paths_by_split[task["split"]].update(row["path"] for row in labels)
        gold_tasks.append(
            {
                **task,
                "labels": labels,
                "unsupported": unsupported,
                "label_state": "unjudged" if unsupported else "mechanical_unreviewed",
                "answerable": None if unsupported else bool(labels),
            }
        )
        blind_tasks.append(
            {
                key: task[key]
                for key in (
                    "task_id",
                    "query_family_id",
                    "intent",
                    "query",
                    "scope_prefix",
                    "language",
                    "case_semantics",
                    "normalization",
                )
            }
        )
    if paths_by_split["development"] & paths_by_split["holdout"]:
        raise EvidenceError("gold-positive file leakage across development and holdout")
    return (
        {"schema_version": 1, "oracle_version": ORACLE_VERSION, "tasks": gold_tasks},
        {"schema_version": 1, "tasks": blind_tasks},
    )
