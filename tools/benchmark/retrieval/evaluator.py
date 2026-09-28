#!/usr/bin/env python3
"""Evaluate externally recorded retrieval routes against frozen repository labels.

The runner consumes only the output of ``freeze``. Gold labels are read only by
``evaluate``. Hashes and a clean, pinned Git checkout bind every scored block
to the repository contents; the evaluator never generates candidate results.

The current suite/query-pack shape is schema v3 and current runner shape is
schema v5. Runner v5 stores a canonical execution profile and digest on each
capture, so routes bound to different systems can carry different query
policies without runner-wide ambiguity. Runner v3/v4 are accepted only as
historical replay inputs; new merges require v5. The evaluator requires a
comparison contract (top_k, tokenizer, budget version, output-unit policy)
on the suite, blinded query pack and every record, with byte-equality
required between all three; per-capture provenance (chunk strategy and
config, runner/searchd binary identity, generation, receipt and activation
digests, model identity and v5 execution profile) preserved under
``captures`` with each route referencing one ``capture_id``; and nullable timings where
unknown latency is null and 0 asserts an actually measured zero. V3
spans are byte spans with a consistency-checked line projection:
coverage is byte containment, and a byte span that disagrees with its
line bytes is refused. Suites require the file universe with a
recomputed digest, per-task query families that must not span splits,
normalized/shingle near-duplicate query refusal, and an explicit
rationale-backed allowlist for any cross-split span overlap. Executed
timeouts must carry their measured duration, and duplicate candidate
byte spans are refused. Unknown artifact stamps are rejected; historical
runner records use explicit versioned validators and do not enter current
record merges.

Freeze output never contains gold spans, grades, answerability bits, or train
tasks. Score computation depends only on recorded candidates and statuses,
never on runner identity or provenance strings.
"""

from __future__ import annotations

import argparse
import json
import math
import random
import re
import subprocess
import sys
from collections.abc import Sequence
from functools import lru_cache
from pathlib import Path, PurePosixPath
from typing import Any

try:
    from tools.benchmark.retrieval import query_plan as query_plan_contract
    from tools.benchmark.retrieval import retrieval_contract
    from tools.benchmark.retrieval.finite_json import is_finite_json_number
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval import query_plan as query_plan_contract
    from tools.benchmark.retrieval import retrieval_contract
    from tools.benchmark.retrieval.finite_json import is_finite_json_number

SCHEMA_VERSION = 3
RUNNER_SCHEMA_VERSION = 5
RUNNER_LEGACY_SCHEMA_VERSIONS = (3, 4)
BUDGETS = (2000, 4000, 8000, 16000)
TOKENIZER = retrieval_contract.TOKENIZER
TOKENIZER_BUDGET_VERSION = retrieval_contract.TOKENIZER_BUDGET_VERSION
# Explicit ASCII ranges keep token accounting stable across Python's Unicode
# database revisions. Non-ASCII code points each count as one benchmark unit.
TOKEN_RE = retrieval_contract.TOKEN_RE
HEX64_RE = re.compile(r"[0-9a-f]{64}\Z")
COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")

RECALL_KS = (1, 5, 10, 20)
MRR_K = 10
NDCG_K = 10
RESULT_STATUSES = ("success", "abstained", "capped", "error", "timeout", "unavailable")
NON_SUCCESS_EMPTY = ("abstained", "error", "timeout", "unavailable")
SCORED_STATUSES = ("success", "capped")
BLINDING_VALUES = ("isolated", "attested")
OUTPUT_UNIT_POLICIES = retrieval_contract.OUTPUT_UNIT_POLICIES
SPAN_UNIT = retrieval_contract.SPAN_UNIT
QUERY_SHINGLE_N = 5
QUERY_NEAR_DUP_JACCARD = 0.8
CHUNK_STRATEGIES = (
    "whole_file",
    "fixed_window_strict",
    "fixed_window_line_aligned",
    "brace_heuristic",
    "semble_native",
)
CAPTURE_SYSTEMS = ("quanta", "semble")
NOT_APPLICABLE = "not_applicable"
MIN_CI_SAMPLE = 20


class EvidenceError(ValueError):
    """Required benchmark evidence is absent, inconsistent or contaminated."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise EvidenceError(message)


def object_keys(value: Any, keys: Sequence[str], where: str) -> dict[str, Any]:
    require(isinstance(value, dict), f"{where} must be an object")
    require(
        set(value) == set(keys),
        f"{where} has missing/unknown fields: {sorted(set(value) ^ set(keys))}",
    )
    return value


def object_keys_optional(
    value: Any, required: Sequence[str], optional: Sequence[str], where: str
) -> dict[str, Any]:
    require(isinstance(value, dict), f"{where} must be an object")
    keys = set(value)
    missing = sorted(set(required) - keys)
    unknown = sorted(keys - set(required) - set(optional))
    require(not missing, f"{where} has missing fields: {missing}")
    require(not unknown, f"{where} has missing/unknown fields: {unknown}")
    return value


def string(value: Any, where: str) -> str:
    require(isinstance(value, str) and bool(value.strip()), f"{where} must be a nonempty string")
    return value


def sha(value: Any, where: str) -> str:
    require(
        isinstance(value, str) and bool(HEX64_RE.fullmatch(value)),
        f"{where} must be a lowercase sha256",
    )
    return value


def positive_int(value: Any, where: str) -> int:
    require(type(value) is int and value > 0, f"{where} must be a positive integer")
    return value


def grade_value(value: Any, where: str) -> int:
    require(type(value) is int and 1 <= value <= 3, f"{where} gold grade must be an integer 1-3")
    return value


def finite_timing(value: Any, where: str) -> float:
    require(
        is_finite_json_number(value) and value >= 0,
        f"{where} timing must be a finite number >= 0",
    )
    return float(value)


def nonnegative_int(value: Any, where: str) -> int:
    require(type(value) is int and value >= 0, f"{where} must be an integer >= 0")
    return value


def strict_bool(value: Any, where: str) -> bool:
    require(type(value) is bool, f"{where} must be a strict boolean")
    return value


def nullable_timing(value: Any, where: str) -> float | None:
    """Null means unknown; 0 asserts an actually measured zero."""
    if value is None:
        return None
    return finite_timing(value, where)


def validate_comparison_contract(value: Any, where: str) -> dict[str, Any]:
    try:
        return retrieval_contract.validate_comparison_contract(value, where)
    except ValueError as exc:
        raise EvidenceError(str(exc)) from exc


def normalize_query(text: str) -> str:
    """Deterministic ASCII query normalization for near-duplicate detection."""
    lowered = text.lower()
    cleaned = "".join(c if "a" <= c <= "z" or "0" <= c <= "9" else " " for c in lowered)
    return " ".join(cleaned.split())


def query_shingles(normalized: str, n: int = QUERY_SHINGLE_N) -> set[str]:
    if not normalized:
        return set()
    if len(normalized) < n:
        return {normalized}
    return {normalized[i : i + n] for i in range(len(normalized) - n + 1)}


def shingle_jaccard(first: set[str], second: set[str]) -> float:
    if not first and not second:
        return 1.0
    if not first or not second:
        return 0.0
    return len(first & second) / len(first | second)


def universe_digest(entries: list[dict[str, str]]) -> str:
    ordered = sorted(entries, key=lambda e: str(e["path"]))
    return digest(canonical(ordered))


def validate_chunk_config(value: Any, where: str) -> dict[str, Any]:
    config = object_keys_optional(
        value,
        [],
        ["window_bytes", "overlap_bytes", "max_item_bytes", "alignment", "byte_cap_strict"],
        where,
    )
    if "window_bytes" in config:
        positive_int(config["window_bytes"], where + ".window_bytes")
    if "overlap_bytes" in config:
        nonnegative_int(config["overlap_bytes"], where + ".overlap_bytes")
    if "max_item_bytes" in config:
        positive_int(config["max_item_bytes"], where + ".max_item_bytes")
    if "alignment" in config:
        require(
            config["alignment"] in ("byte", "line"),
            where + ".alignment must be byte or line",
        )
    if "byte_cap_strict" in config:
        strict_bool(config["byte_cap_strict"], where + ".byte_cap_strict")
    return config


def validate_execution_profile(value: Any, system: str, where: str) -> dict[str, Any]:
    require(isinstance(value, dict), f"{where} must be an object")
    if system == "quanta":
        profile = object_keys(
            value,
            ["profile_id", "policy", "config", "planning_cost_in_latency"],
            where,
        )
        policy = profile["policy"]
        require(policy in query_plan_contract.SUPPORTED_POLICIES, f"{where}.policy is unknown")
        config = profile["config"]
        require(isinstance(config, dict), f"{where}.config must be an object")
        expected = query_plan_contract.execution_profile(policy)
        require(profile == expected, f"{where} differs from the frozen Quanta profile")
        return profile
    profile = object_keys(value, ["profile_id", "mode", "alpha", "rerank"], where)
    modes = {
        "native-default": ("semble-native-default-v1", None, "upstream-content-default"),
        "lexical-only": ("semble-lexical-only-v1", None, "not_applicable"),
        "semantic-only": ("semble-semantic-only-v1", None, "not_applicable"),
    }
    mode = profile["mode"]
    require(isinstance(mode, str), f"{where}.mode must be a string")
    if mode == "hybrid-no-rerank":
        require(
            profile["profile_id"] == "semble-hybrid-no-rerank-v1", f"{where}.profile_id mismatch"
        )
        alpha = profile["alpha"]
        require(is_finite_json_number(alpha) and 0 <= alpha <= 1, f"{where}.alpha is invalid")
        require(profile["rerank"] is False, f"{where}.rerank must be false")
    else:
        require(mode in modes, f"{where}.mode is unknown")
        expected_id, alpha, rerank = modes[mode]
        require(
            profile == {"profile_id": expected_id, "mode": mode, "alpha": alpha, "rerank": rerank},
            f"{where} differs from the frozen Semble profile",
        )
    return profile


def validate_capture(
    value: Any, where: str, version: int = RUNNER_SCHEMA_VERSION
) -> dict[str, Any]:
    fields = [
        "system",
        "chunk_strategy",
        "chunk_config",
        "runner_binary",
        "searchd_binary",
        "generation",
        "receipt_digest",
        "activation_digest",
        "model",
        "model_revision",
    ]
    if version == 5:
        fields.extend(["execution_profile", "execution_profile_sha256"])
    capture = object_keys(
        value,
        fields,
        where,
    )
    system = capture["system"]
    require(system in CAPTURE_SYSTEMS, f"{where}.system must be quanta or semble")
    require(
        capture["chunk_strategy"] in CHUNK_STRATEGIES,
        f"{where}.chunk_strategy is not a frozen v3 strategy",
    )
    validate_chunk_config(capture["chunk_config"], where + ".chunk_config")
    binary = object_keys(capture["runner_binary"], ["name", "digest"], where + ".runner_binary")
    string(binary["name"], where + ".runner_binary.name")
    sha(binary["digest"], where + ".runner_binary.digest")
    searchd = capture["searchd_binary"]
    if system == "semble":
        require(searchd is None, f"{where}.searchd_binary must be null for semble captures")
    else:
        pinned = object_keys(searchd, ["binary_digest"], where + ".searchd_binary")
        sha(pinned["binary_digest"], where + ".searchd_binary.binary_digest")
    generation = nonnegative_int(capture["generation"], where + ".generation")
    if system == "semble":
        require(generation == 0, f"{where}.generation must be 0 for semble captures")
    else:
        require(generation > 0, f"{where}.generation must be positive for quanta captures")
    sha(capture["receipt_digest"], where + ".receipt_digest")
    sha(capture["activation_digest"], where + ".activation_digest")
    string(capture["model"], where + ".model")
    string(capture["model_revision"], where + ".model_revision")
    if version == 5:
        profile = validate_execution_profile(
            capture["execution_profile"], system, where + ".execution_profile"
        )
        require(
            sha(capture["execution_profile_sha256"], where + ".execution_profile_sha256")
            == digest(canonical(profile)),
            f"{where}.execution_profile_sha256 mismatch",
        )
    return capture


def digest(data: bytes) -> str:
    return retrieval_contract.digest(data)


def canonical(value: Any) -> bytes:
    return retrieval_contract.canonical(value)


def read_json(path: Path) -> Any:
    def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            require(key not in result, "duplicate JSON key: " + key)
            result[key] = value
        return result

    def reject_constant(value: str) -> Any:
        raise EvidenceError("non-finite JSON number: " + value)

    try:
        return json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=unique_object,
            parse_constant=reject_constant,
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise EvidenceError(f"cannot read JSON {path}: {exc}") from exc


def git(repo: Path, *args: str) -> str:
    try:
        result = subprocess.run(
            ["git", "-C", str(repo), *args], check=True, capture_output=True, text=True
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        raise EvidenceError(f"repository Git evidence unavailable: {exc}") from exc
    return result.stdout.strip()


def verify_repo(repo: Path, commit: str) -> Path:
    try:
        return retrieval_contract.verify_repo(repo, commit)
    except ValueError as exc:
        raise EvidenceError(str(exc)) from exc


class SourceSnapshot:
    """A clean commit's tracked path inventory and lazily cached source bytes."""

    def __init__(self, repo: Path, commit: str) -> None:
        self.repo = verify_repo(repo, commit)
        try:
            listing = subprocess.run(
                ["git", "-C", str(self.repo), "ls-files", "-z"],
                check=True,
                capture_output=True,
            ).stdout.decode("utf-8")
        except (OSError, UnicodeError, subprocess.CalledProcessError) as exc:
            raise EvidenceError(f"tracked source inventory unavailable: {exc}") from exc
        self.tracked = set(listing.rstrip("\0").split("\0"))
        self.files: dict[str, tuple[bytes, list[bytes], str]] = {}

    def file(self, name: str) -> tuple[bytes, list[bytes], str]:
        if name not in self.files:
            absolute = safe_path(self, name)
            raw = absolute.read_bytes()
            self.files[name] = (raw, raw.splitlines(keepends=True), digest(raw))
        return self.files[name]


def safe_path(source: SourceSnapshot, raw: Any) -> Path:
    name = string(raw, "block.path")
    path = PurePosixPath(name)
    require(
        not path.is_absolute()
        and len(path.parts) > 0
        and all(part not in (".", "..") for part in path.parts),
        f"unsafe repository path: {name}",
    )
    require(path.as_posix() == name and "\\" not in name, f"noncanonical repository path: {name}")
    absolute = (source.repo / name).resolve()
    require(source.repo in absolute.parents, f"block path escapes repository: {name}")
    require(absolute.is_file(), f"repository file missing: {name}")
    require(not (source.repo / name).is_symlink(), f"symlink is not source-file evidence: {name}")
    require(name in source.tracked, f"block file is not tracked by frozen commit: {name}")
    return absolute


def block(
    source: SourceSnapshot,
    value: Any,
    where: str,
    *,
    candidate: bool,
    universe: set[str] | None = None,
    allow_grade: bool = False,
    allow_span_accounting: bool = False,
) -> dict[str, Any]:
    required = [
        "path",
        "start_line",
        "end_line",
        "start_byte",
        "end_byte",
        "file_sha256",
        "block_sha256",
    ]
    if candidate:
        required.extend(["tokens", "rank"])
    optional = ["grade"] if (allow_grade and not candidate) else []
    if candidate and allow_span_accounting:
        optional.append("span_accounting")
    if optional:
        item = object_keys_optional(value, required, optional, where)
    else:
        item = object_keys(value, required, where)
    path = string(item["path"], where + ".path")
    start = positive_int(item["start_line"], where + ".start_line")
    end = positive_int(item["end_line"], where + ".end_line")
    require(start <= end, where + " has inverted line span")
    raw, lines, file_digest = source.file(path)
    require(end <= len(lines), where + " spans beyond EOF")
    require(
        sha(item["file_sha256"], where + ".file_sha256") == file_digest,
        where + " file hash mismatch",
    )
    if universe is not None:
        require(path in universe, where + f" file excluded from file universe: {path}")
    start_byte = nonnegative_int(item["start_byte"], where + ".start_byte")
    end_byte = positive_int(item["end_byte"], where + ".end_byte")
    require(end_byte > start_byte, where + " has an empty byte span")
    require(end_byte <= len(raw), where + " byte span runs past EOF")
    selected = raw[start_byte:end_byte]
    try:
        text = selected.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise EvidenceError(f"block byte span cuts a UTF-8 boundary: {where}") from exc
    projected = b"".join(lines[start - 1 : end])
    require(
        selected == projected,
        where + " byte span disagrees with its line projection",
    )
    block_digest = digest(selected)
    count = len(TOKEN_RE.findall(text))
    require(count > 0, where + " block contains no retrievable tokens")
    require(
        sha(item["block_sha256"], where + ".block_sha256") == block_digest,
        where + " block hash mismatch",
    )
    if candidate:
        require(
            positive_int(item["tokens"], where + ".tokens") == count,
            where + " token count mismatch",
        )
    if candidate:
        positive_int(item["rank"], where + ".rank")
        if "span_accounting" in item:
            accounting = object_keys(
                item["span_accounting"],
                [
                    "unit_kind",
                    "unit_id",
                    "producer_identity",
                    "indexed_start_byte",
                    "indexed_end_byte",
                    "sdk_start_line",
                    "sdk_end_line",
                    "extra_context_bytes",
                ],
                where + ".span_accounting",
            )
            require(
                accounting["unit_kind"] in ("chunk", "symbol"), where + " has unknown unit kind"
            )
            string(accounting["unit_id"], where + ".unit_id")
            string(accounting["producer_identity"], where + ".producer_identity")
            indexed_start = nonnegative_int(
                accounting["indexed_start_byte"], where + ".indexed_start_byte"
            )
            indexed_end = positive_int(accounting["indexed_end_byte"], where + ".indexed_end_byte")
            require(
                start_byte <= indexed_start < indexed_end <= end_byte,
                where + " indexed span escapes scored projection",
            )
            try:
                raw[indexed_start:indexed_end].decode("utf-8")
            except UnicodeDecodeError as exc:
                raise EvidenceError(where + " indexed span cuts a UTF-8 boundary") from exc
            sdk_start = nonnegative_int(accounting["sdk_start_line"], where + ".sdk_start_line")
            sdk_end = nonnegative_int(accounting["sdk_end_line"], where + ".sdk_end_line")
            require(
                (sdk_start == sdk_end == 0 and accounting["unit_kind"] == "chunk")
                or (sdk_start == start and sdk_end == end),
                where + " SDK line span differs from scored projection",
            )
            require(
                nonnegative_int(accounting["extra_context_bytes"], where + ".extra_context_bytes")
                == (end_byte - start_byte) - (indexed_end - indexed_start),
                where + " context expansion differs from source spans",
            )
    if "grade" in item:
        grade_value(item["grade"], where)
    return item


def validate_file_universe(
    source: SourceSnapshot, value: Any
) -> tuple[dict[str, str], list[dict[str, str]]]:
    require(isinstance(value, list) and bool(value), "file_universe must be a nonempty list")
    entries: dict[str, str] = {}
    for entry in value:
        item = object_keys(entry, ["path", "file_sha256"], "file_universe entry")
        path = string(item["path"], "file_universe entry.path")
        safe_path(source, path)
        _, _, file_digest = source.file(path)
        require(
            sha(item["file_sha256"], "file_universe entry.file_sha256") == file_digest,
            f"file universe file hash mismatch: {path}",
        )
        require(path not in entries, "duplicate file_universe path: " + path)
        entries[path] = file_digest
    ordered = [{"path": p, "file_sha256": entries[p]} for p in sorted(entries)]
    return entries, ordered


def validate_leakage_allowlist(
    source: SourceSnapshot, value: Any
) -> frozenset[tuple[str, int, int]]:
    require(isinstance(value, list), "leakage_allowlist must be a list")
    allowed: set[tuple[str, int, int]] = set()
    for entry in value:
        item = object_keys(
            entry,
            ["path", "start_line", "end_line", "rationale_digest"],
            "leakage allowlist entry",
        )
        path = string(item["path"], "allowlist entry.path")
        safe_path(source, path)
        _raw, lines, _digest = source.file(path)
        start = positive_int(item["start_line"], "allowlist entry.start_line")
        end = positive_int(item["end_line"], "allowlist entry.end_line")
        require(start <= end, "allowlist entry has an inverted line span")
        require(end <= len(lines), "allowlist entry spans beyond EOF")
        sha(item["rationale_digest"], "allowlist entry.rationale_digest")
        key = (path, start, end)
        require(key not in allowed, "duplicate leakage allowlist entry")
        allowed.add(key)
    return frozenset(allowed)


def check_query_near_duplicates(queries: list[tuple[str, str]]) -> None:
    normalized = [(task_id, normalize_query(query)) for task_id, query in queries]
    seen: dict[str, str] = {}
    for task_id, text in normalized:
        if text in seen:
            raise EvidenceError(
                "query leakage/duplication across tasks (normalized match): "
                + task_id
                + " vs "
                + seen[text]
            )
        seen[text] = task_id
    shingles = [(task_id, query_shingles(text)) for task_id, text in normalized]
    for index, (task_id, grams) in enumerate(shingles):
        for other_id, other_grams in shingles[index + 1 :]:
            score = shingle_jaccard(grams, other_grams)
            require(
                score < QUERY_NEAR_DUP_JACCARD,
                "query near-duplicate across tasks: "
                + task_id
                + " vs "
                + other_id
                + f" (shingle jaccard {score:.3f})",
            )


def _check_split_leakage(
    labels_by_split: dict[str, set[tuple[str, int, int]]],
    allowlist: frozenset[tuple[str, int, int]] = frozenset(),
) -> None:
    by_path: dict[str, dict[str, list[tuple[int, int]]]] = {}
    for split, labels in labels_by_split.items():
        for path, start, end in labels:
            by_path.setdefault(path, {}).setdefault(split, []).append((start, end))
    for path, splits in by_path.items():
        for train in splits.get("train", []):
            for eval_span in splits.get("eval", []):
                overlap = train[0] <= eval_span[1] and eval_span[0] <= train[1]
                if not overlap:
                    continue
                allowed = (path, *train) in allowlist and (path, *eval_span) in allowlist
                require(
                    allowed,
                    "gold label leakage across train/eval split: "
                    f"{path}:{train[0]}-{train[1]} vs eval {eval_span[0]}-{eval_span[1]}",
                )


def validate_suite(
    repo: Path, payload: Any
) -> tuple[dict[str, Any], dict[str, Any], SourceSnapshot]:
    version = payload.get("schema_version") if isinstance(payload, dict) else None
    require(type(version) is int and version == SCHEMA_VERSION, "unsupported suite schema")
    required = [
        "schema_version",
        "suite_id",
        "repository_commit",
        "comparison_contract",
        "routes",
        "tasks",
        "file_universe",
        "file_universe_digest",
    ]
    suite = object_keys_optional(
        payload,
        required,
        ["leakage_allowlist"],
        "suite",
    )
    require(
        type(suite["schema_version"]) is int and suite["schema_version"] == SCHEMA_VERSION,
        "unsupported suite schema",
    )
    validate_comparison_contract(suite["comparison_contract"], "suite.comparison_contract")
    string(suite["suite_id"], "suite_id")
    commit = suite["repository_commit"]
    require(
        isinstance(commit, str) and bool(COMMIT_RE.fullmatch(commit)),
        "repository_commit must be a full lowercase Git SHA",
    )
    source = SourceSnapshot(repo, commit)
    routes = suite["routes"]
    require(isinstance(routes, list) and len(routes) >= 1, "suite requires at least one route")
    for route in routes:
        string(route, "route")
    require(len(set(routes)) == len(routes), "duplicate route")
    entries, ordered_universe = validate_file_universe(source, suite["file_universe"])
    universe = set(entries)
    require(
        sha(suite["file_universe_digest"], "file_universe_digest")
        == universe_digest(ordered_universe),
        "file universe digest mismatch",
    )
    allowlist: frozenset[tuple[str, int, int]] = frozenset()
    if "leakage_allowlist" in suite:
        allowlist = validate_leakage_allowlist(source, suite["leakage_allowlist"])
    tasks = suite["tasks"]
    require(isinstance(tasks, list) and bool(tasks), "suite requires tasks")
    seen_ids = set()
    seen_queries = set()
    eval_count = 0
    families: dict[str, set[str]] = {}
    queries: list[tuple[str, str]] = []
    labels_by_split: dict[str, set[tuple[str, int, int]]] = {"train": set(), "eval": set()}
    task_required = [
        "task_id",
        "split",
        "query",
        "query_sha256",
        "query_family_id",
        "answerable",
        "gold",
    ]
    for raw in tasks:
        task = object_keys_optional(
            raw,
            task_required,
            ["category"],
            "task",
        )
        task_id = string(task["task_id"], "task_id")
        require(task_id not in seen_ids, "duplicate task_id: " + task_id)
        seen_ids.add(task_id)
        require(task["split"] in ("train", "eval"), "invalid task split: " + task_id)
        if "category" in task:
            string(task["category"], "category for " + task_id)
        query = string(task["query"], "query")
        query_hash = sha(task["query_sha256"], "query_sha256")
        require(digest(query.encode("utf-8")) == query_hash, "query hash mismatch: " + task_id)
        require(
            query_hash not in seen_queries, "query leakage/duplication across tasks: " + task_id
        )
        seen_queries.add(query_hash)
        queries.append((task_id, query))
        family = string(task["query_family_id"], "query_family_id for " + task_id)
        families.setdefault(family, set()).add(task["split"])
        require(type(task["answerable"]) is bool, "answerable must be boolean: " + task_id)
        labels = task["gold"]
        require(isinstance(labels, list), "gold must be a list: " + task_id)
        require(task["answerable"] == bool(labels), "answerable/gold mismatch: " + task_id)
        seen_labels = set()
        for label in labels:
            block(
                source,
                label,
                "gold label for " + task_id,
                candidate=False,
                universe=universe,
                allow_grade=True,
            )
            key = (label["path"], label["start_line"], label["end_line"])
            require(key not in seen_labels, "duplicate gold label: " + task_id)
            seen_labels.add(key)
            labels_by_split[task["split"]].add(key)
        if task["split"] == "eval":
            eval_count += 1
    require(eval_count > 0, "eval split requires at least one task")
    for family, splits in sorted(families.items()):
        require(
            len(splits) == 1,
            f"query family spans train and eval: {family}",
        )
    check_query_near_duplicates(queries)
    _check_split_leakage(labels_by_split, allowlist)
    blinded = {
        "schema_version": SCHEMA_VERSION,
        "suite_id": suite["suite_id"],
        "suite_commitment_sha256": digest(canonical(suite)),
        "repository_commit": commit,
        "tokenizer": TOKENIZER,
        "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
        "routes": routes,
        "file_universe": ordered_universe,
        "tasks": [
            {
                "task_id": task["task_id"],
                "query": task["query"],
                "query_sha256": task["query_sha256"],
            }
            for task in tasks
            if task["split"] == "eval"
        ],
    }
    blinded["comparison_contract"] = suite["comparison_contract"]
    blinded["file_universe_digest"] = suite["file_universe_digest"]
    return suite, blinded, source


def validate_experiment_custody(
    repo: Path,
    manifest: Any,
    development: Any,
    holdout: Any,
) -> dict[str, Any]:
    """Re-derive the frozen cross-suite development/holdout boundary.

    This is separate from the historical suite-v3 intra-suite train/eval
    contract. A gold-bearing file is indivisible for this boundary: distinct
    gold spans in the same source file still leak development context into
    holdout. Both suites may index the same corpus universe.
    """
    record = object_keys(
        manifest,
        [
            "schema_version",
            "source_revision",
            "repository_commit",
            "development_suite_sha256",
            "holdout_suite_sha256",
        ],
        "experiment custody",
    )
    require(
        type(record["schema_version"]) is int and record["schema_version"] == 1,
        "unsupported experiment custody schema",
    )
    require(
        isinstance(record["source_revision"], str)
        and bool(COMMIT_RE.fullmatch(record["source_revision"])),
        "experiment source_revision must be a full Git SHA",
    )
    require(
        isinstance(record["repository_commit"], str)
        and bool(COMMIT_RE.fullmatch(record["repository_commit"])),
        "experiment repository_commit must be a full Git SHA",
    )
    dev_suite, _dev_pack, _dev_source = validate_suite(repo, development)
    holdout_suite, _holdout_pack, _holdout_source = validate_suite(repo, holdout)
    require(
        dev_suite["suite_id"] != holdout_suite["suite_id"],
        "development and holdout suite IDs coincide",
    )
    require(
        dev_suite["repository_commit"]
        == holdout_suite["repository_commit"]
        == record["repository_commit"],
        "cross-suite repository commit mismatch",
    )
    require(
        dev_suite["comparison_contract"] == holdout_suite["comparison_contract"]
        and dev_suite["routes"] == holdout_suite["routes"],
        "cross-suite scoring or route contract differs",
    )
    for label, suite in (("development", dev_suite), ("holdout", holdout_suite)):
        require(
            sha(record[f"{label}_suite_sha256"], f"{label}_suite_sha256")
            == digest(canonical(suite)),
            f"{label} suite differs from frozen experiment custody",
        )
    dev_files = {gold["path"] for task in dev_suite["tasks"] for gold in task["gold"]}
    holdout_files = {gold["path"] for task in holdout_suite["tasks"] for gold in task["gold"]}
    require(
        not (dev_files & holdout_files),
        f"cross-suite file leakage: {sorted(dev_files & holdout_files)}",
    )
    dev_families = {task["query_family_id"] for task in dev_suite["tasks"]}
    holdout_families = {task["query_family_id"] for task in holdout_suite["tasks"]}
    require(
        not (dev_families & holdout_families),
        f"cross-suite query family leakage: {sorted(dev_families & holdout_families)}",
    )
    dev_blocks = {gold["block_sha256"] for task in dev_suite["tasks"] for gold in task["gold"]}
    holdout_blocks = {
        gold["block_sha256"] for task in holdout_suite["tasks"] for gold in task["gold"]
    }
    require(
        not (dev_blocks & holdout_blocks),
        "cross-suite gold definition content leakage",
    )
    check_query_near_duplicates(
        [(f"development/{task['task_id']}", task["query"]) for task in dev_suite["tasks"]]
        + [(f"holdout/{task['task_id']}", task["query"]) for task in holdout_suite["tasks"]]
    )
    return record


def _validate_run(
    run: dict[str, Any],
    pack: dict[str, Any],
    suite: dict[str, Any],
    source: SourceSnapshot,
) -> dict[str, Any]:
    version = run["schema_version"]
    require(
        type(version) is int and version in (RUNNER_SCHEMA_VERSION, *RUNNER_LEGACY_SCHEMA_VERSIONS),
        "unsupported runner schema",
    )
    span_protocol = run.get("span_accounting_version")
    require(
        span_protocol is None
        or (version == RUNNER_SCHEMA_VERSION and type(span_protocol) is int and span_protocol == 1),
        "unsupported span accounting protocol",
    )
    require(
        sha(run["query_pack_sha256"], "query_pack_sha256") == digest(canonical(pack)),
        "runner query pack hash mismatch",
    )
    contract = validate_comparison_contract(
        run["comparison_contract"], "record.comparison_contract"
    )
    pack_contract = pack.get("comparison_contract")
    require(
        isinstance(pack_contract, dict) and pack_contract == contract,
        "record comparison contract differs from the query-pack contract",
    )
    runner_keys = [
        "name",
        "revision",
        "run_id",
        "tokenizer",
        "tokenizer_budget_version",
        "gold_access",
        "blinding",
        "isolation_method",
        "access_block_log",
    ]
    if version == 4:
        runner_keys.append("query_input_policy")
    runner = object_keys(run["runner"], runner_keys, "runner")
    for key in ("name", "revision", "run_id"):
        string(runner[key], "runner." + key)
    require(runner["tokenizer"] == TOKENIZER, "runner tokenizer mismatch")
    require(
        runner["tokenizer_budget_version"] == TOKENIZER_BUDGET_VERSION,
        "runner tokenizer/budget version mismatch",
    )
    require(
        runner["gold_access"] is False, "runner attests gold access or lacks no-gold attestation"
    )
    require(runner["blinding"] in BLINDING_VALUES, "runner blinding must be isolated or attested")
    string(runner["isolation_method"], "runner.isolation_method")
    string(runner["access_block_log"], "runner.access_block_log")
    policy = None
    nl_config = None
    if version == 4:
        policy_block = object_keys(
            runner["query_input_policy"],
            ["policy", "config", "policy_config_sha256", "planning_cost_in_latency"],
            "runner.query_input_policy",
        )
        policy = policy_block["policy"]
        require(
            policy in query_plan_contract.V4_SUPPORTED_POLICIES,
            f"unknown query input policy: {policy!r}",
        )
        config = policy_block["config"]
        require(isinstance(config, dict), "runner.query_input_policy.config must be an object")
        if policy == "natural_language":
            nl_config = object_keys(
                config,
                ["max_token_chars", "max_tokens", "min_token_chars"],
                "runner.query_input_policy.config",
            )
            for field in ("max_token_chars", "max_tokens", "min_token_chars"):
                require(
                    type(nl_config[field]) is int and nl_config[field] >= 1,
                    f"runner.query_input_policy.config.{field} must be a positive integer",
                )
        else:
            require(
                not config,
                "runner.query_input_policy.config must be empty unless natural_language",
            )
        require(
            sha(policy_block["policy_config_sha256"], "policy_config_sha256")
            == digest(query_plan_contract.policy_config_canonical_v4(policy, nl_config).encode()),
            "policy config digest does not match the canonical policy profile",
        )
        require(
            policy_block["planning_cost_in_latency"]
            is query_plan_contract.PLANNING_COST_IN_LATENCY,
            "planning_cost_in_latency disagrees with the frozen profile contract",
        )
    captures = run["captures"]
    require(isinstance(captures, dict) and bool(captures), "captures must be a nonempty object")
    for capture_id, entry in captures.items():
        require(
            isinstance(capture_id, str) and bool(capture_id.strip()),
            "capture_id must be a nonempty string",
        )
        validate_capture(entry, f"captures.{capture_id}", version)
    if span_protocol == 1:
        require(
            any(entry["system"] == "quanta" for entry in captures.values()),
            "span accounting protocol lacks a Quanta capture",
        )
    provenance = run["route_provenance"]
    require(isinstance(provenance, dict), "route_provenance must be an object")
    require(
        set(provenance) == set(suite["routes"]),
        f"route_provenance has missing/unknown routes: {sorted(set(provenance) ^ set(suite['routes']))}",
    )
    for route, entry in provenance.items():
        item = object_keys(entry, ["capture_id"], f"route_provenance.{route}")
        capture_id = string(item["capture_id"], f"route_provenance.{route}.capture_id")
        require(
            capture_id in captures,
            f"route_provenance.{route} references unknown capture_id: {capture_id}",
        )
        capture = captures[capture_id]
        if (
            version == 5
            and capture["system"] == "quanta"
            and capture["execution_profile"]["policy"] == "exact_symbol_name"
        ):
            require(route == "symbol", "exact_symbol_name profile requires the symbol route")
    universe: set[str] | None = None
    if "file_universe" in suite:
        universe = {entry["path"] for entry in suite["file_universe"]}
    results = run["results"]
    require(isinstance(results, list), "results must be a list")
    tasks = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
    expected = {(task_id, route) for task_id in tasks for route in suite["routes"]}
    result_keys = ["task_id", "route", "status", "candidates", "timings", "error"]
    if version in (4, 5):
        result_keys.insert(4, "query_identity")
    pack_queries = {task["task_id"]: task for task in pack["tasks"]}
    found = set()
    for raw in results:
        result = object_keys(raw, result_keys, "result")
        key = (string(result["task_id"], "result.task_id"), string(result["route"], "result.route"))
        require(key in expected and key not in found, f"unexpected/duplicate task route: {key}")
        found.add(key)
        if version in (4, 5):
            pack_task = pack_queries.get(key[0])
            require(
                pack_task is not None,
                f"query identity refers to unknown pack task: {key}",
            )
            if version == 4:
                identity = object_keys(
                    result["query_identity"],
                    [
                        "original_query_sha256",
                        "effective_lexical_request_sha256",
                        "semantic_text_sha256",
                    ],
                    f"query_identity for {key}",
                )
                expected_identity = query_plan_contract.derive_query_identity_v4(
                    policy, pack_task["query"], nl_config
                )
            else:
                capture_id = provenance[key[1]]["capture_id"]
                capture = captures[capture_id]
                profile = capture["execution_profile"]
                if capture["system"] == "quanta":
                    identity = object_keys(
                        result["query_identity"],
                        [
                            "original_query_sha256",
                            "effective_lexical_request_sha256",
                            "semantic_text_sha256",
                        ],
                        f"query_identity for {key}",
                    )
                    expected_identity = query_plan_contract.derive_query_identity(
                        profile["policy"], pack_task["query"], profile["config"] or None
                    )
                else:
                    identity = object_keys(
                        result["query_identity"],
                        ["original_query_sha256", "submitted_query_sha256"],
                        f"query_identity for {key}",
                    )
                    raw_sha = digest(pack_task["query"].encode())
                    expected_identity = {
                        "original_query_sha256": raw_sha,
                        "submitted_query_sha256": raw_sha,
                    }
            require(
                {field: sha(identity[field], f"{field} for {key}") for field in identity}
                == expected_identity,
                f"query identity does not match the independently re-derived plan: {key}",
            )
            require(
                identity["original_query_sha256"] == pack_task["query_sha256"],
                f"original query digest differs from the frozen pack task digest: {key}",
            )
        status = result["status"]
        require(status in RESULT_STATUSES, f"unknown result status for {key}: {status!r}")
        candidates = result["candidates"]
        require(isinstance(candidates, list), f"candidates must be a list: {key}")
        timings = object_keys(result["timings"], ["query_latency_ms"], f"timings for {key}")
        measured = nullable_timing(timings["query_latency_ms"], f"timings for {key}")
        if status == "timeout":
            require(
                measured is not None,
                f"executed timeout must carry its measured duration: {key}",
            )
        error = result["error"]
        if status in SCORED_STATUSES:
            require(error is None, f"error must be null for {status} result: {key}")
            require(bool(candidates), f"empty non-abstaining result: {key}")
        else:
            require(not candidates, f"non-success result cannot contain candidates: {key}")
            if status == "abstained":
                require(error is None, f"error must be null for abstained result: {key}")
            else:
                item = object_keys(error, ["code", "message"], f"error for {key}")
                string(item["code"], f"error.code for {key}")
                string(item["message"], f"error.message for {key}")
        seen_spans = set()
        seen_unit_ids: set[str] = set()
        for index, candidate in enumerate(candidates, start=1):
            capture = captures[provenance[key[1]]["capture_id"]]
            block(
                source,
                candidate,
                f"candidate for {key}",
                candidate=True,
                universe=universe,
                allow_span_accounting=version == 5 and capture["system"] == "quanta",
            )
            if "span_accounting" in candidate:
                require(span_protocol == 1, f"span evidence lacks record protocol: {key}")
                accounting = candidate["span_accounting"]
                expected_producer = (
                    "source-bound-symbols-v2"
                    if accounting["unit_kind"] == "symbol"
                    else capture["chunk_strategy"]
                )
                require(
                    accounting["producer_identity"] == expected_producer,
                    f"published unit producer differs from capture: {key}",
                )
                require(
                    accounting["unit_id"] not in seen_unit_ids,
                    f"duplicate published unit ID in result: {key}",
                )
                seen_unit_ids.add(accounting["unit_id"])
            elif span_protocol == 1 and capture["system"] == "quanta":
                raise EvidenceError(f"missing published-unit span evidence: {key}")
            require(
                candidate["rank"] == index,
                f"duplicate/non-sequential candidate rank for {key}: expected {index}",
            )
            span = (
                candidate["path"],
                candidate["start_byte"],
                candidate["end_byte"],
            )
            require(span not in seen_spans, f"duplicate candidate byte span: {key}")
            seen_spans.add(span)
    require(found == expected, f"missing task route evidence: {sorted(expected - found)}")
    return run


def load_evidence(
    repo: Path, suite_path: Path, runner_path: Path
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    suite, pack, source = validate_suite(repo, read_json(suite_path))
    payload = read_json(runner_path)
    version = payload.get("schema_version") if isinstance(payload, dict) else None
    require(
        type(version) is int and version in (RUNNER_SCHEMA_VERSION, *RUNNER_LEGACY_SCHEMA_VERSIONS),
        "unsupported runner schema",
    )
    run = object_keys_optional(
        payload,
        [
            "schema_version",
            "query_pack_sha256",
            "comparison_contract",
            "runner",
            "captures",
            "route_provenance",
            "results",
        ],
        ["span_accounting_version"] if version == RUNNER_SCHEMA_VERSION else [],
        "runner record",
    )
    _validate_run(run, pack, suite, source)
    verify_repo(repo, suite["repository_commit"])
    return suite, pack, run


def selected(candidates: list[dict[str, Any]], budget: int) -> tuple[list[dict[str, Any]], int]:
    chosen = []
    used = 0
    for candidate in candidates:
        if used + candidate["tokens"] > budget:
            break  # preserve rank-prefix semantics
        chosen.append(candidate)
        used += candidate["tokens"]
    return chosen, used


def covers(candidate: dict[str, Any], label: dict[str, Any]) -> bool:
    if candidate["path"] != label["path"]:
        return False
    # Line spans are only a checked projection; partial bytes earn no credit.
    return (
        candidate["start_byte"] <= label["start_byte"]
        and candidate["end_byte"] >= label["end_byte"]
    )


def collapse_by_file(candidates: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Deterministic same-file collapse: keep the best-ranked chunk per file."""
    seen: set[str] = set()
    collapsed = []
    for candidate in candidates:
        if candidate["path"] not in seen:
            seen.add(candidate["path"])
            collapsed.append(candidate)
    return collapsed


def recall_at_k(candidates: list[dict[str, Any]], labels: list[dict[str, Any]], k: int) -> float:
    top = candidates[:k]
    hits = sum(any(covers(item, label) for item in top) for label in labels)
    return hits / len(labels)


def mrr_at_k(candidates: list[dict[str, Any]], labels: list[dict[str, Any]], k: int) -> float:
    for index, item in enumerate(candidates[:k], start=1):
        if any(covers(item, label) for label in labels):
            return 1.0 / index
    return 0.0


def indexed_covers(candidate: dict[str, Any], label: dict[str, Any]) -> bool:
    """Exact published-unit coverage, never the SDK's line projection."""
    span = candidate["span_accounting"]
    return (
        candidate["path"] == label["path"]
        and span["indexed_start_byte"] <= label["start_byte"]
        and span["indexed_end_byte"] >= label["end_byte"]
    )


def indexed_span_diagnostics(
    run: dict[str, Any],
    results: dict[tuple[str, str], dict[str, Any]],
    tasks: dict[str, dict[str, Any]],
) -> dict[str, Any] | None:
    """Separate rank-only indexed-span and returned-context accounting.

    Absence in an old record leaves its historical report byte-for-byte
    unchanged. Once any current candidate carries this evidence, partial
    Quanta evidence is an error rather than a silently reduced sample.
    """
    if run.get("span_accounting_version") != 1:
        return None
    answerable = sorted(task_id for task_id, task in tasks.items() if task["gold"])
    routes: dict[str, Any] = {}
    rows: list[dict[str, Any]] = []
    candidate_rows: list[dict[str, Any]] = []
    for route in sorted(run["route_provenance"]):
        capture_id = run["route_provenance"][route]["capture_id"]
        if run["captures"][capture_id]["system"] != "quanta":
            routes[route] = {"status": "not_applicable", "reason": "no_published_unit_authority"}
            continue
        for (task_id, result_route), result in results.items():
            if result_route == route:
                require(
                    all("span_accounting" in item for item in result["candidates"]),
                    f"partial indexed span evidence: {task_id}/{route}",
                )
                for item in result["candidates"]:
                    span = item["span_accounting"]
                    indexed_bytes = span["indexed_end_byte"] - span["indexed_start_byte"]
                    scored_bytes = item["end_byte"] - item["start_byte"]
                    require(
                        indexed_bytes > 0 and scored_bytes >= indexed_bytes,
                        f"invalid candidate span accounting: {task_id}/{route}",
                    )
                    sdk_start = span["sdk_start_line"]
                    sdk_end = span["sdk_end_line"]
                    require(
                        (sdk_start == sdk_end == 0) or (sdk_start > 0 and sdk_start <= sdk_end),
                        f"invalid SDK line span: {task_id}/{route}",
                    )
                    sdk_bytes = scored_bytes if sdk_start > 0 else None
                    candidate_rows.append(
                        {
                            "task_id": task_id,
                            "route": route,
                            "rank": item["rank"],
                            "unit_id": span["unit_id"],
                            "indexed_bytes": indexed_bytes,
                            "sdk_line_span_bytes": sdk_bytes,
                            "scored_projection_bytes": scored_bytes,
                            "scored_projection_tokens": item["tokens"],
                            "scored_to_indexed_expansion_ratio": scored_bytes / indexed_bytes,
                            "sdk_to_indexed_expansion_ratio": (
                                sdk_bytes / indexed_bytes if sdk_bytes is not None else None
                            ),
                        }
                    )
        if not answerable:
            routes[route] = {"status": "not_applicable", "reason": "no_answerable_tasks"}
            continue
        if any(
            _result_status(results[(task_id, route)]) not in SCORED_STATUSES
            for task_id in answerable
        ):
            routes[route] = {"status": "not_run", "reason": "incomplete_answerable_observation"}
            continue
        sums = {
            "rank_only_hit_at_1": 0.0,
            "exact_index_span_mrr_at_10": 0.0,
            "exact_index_span_recall_at_10": 0.0,
            "scored_context_bytes_at_10": 0.0,
            "scored_context_tokens_at_10": 0.0,
            "indexed_bytes_at_10": 0.0,
            "extra_context_bytes_at_10": 0.0,
        }
        for task_id in answerable:
            labels = tasks[task_id]["gold"]
            top = _ordered_candidates(results[(task_id, route)])[:10]
            metrics = {
                "rank_only_hit_at_1": float(
                    bool(top) and any(indexed_covers(top[0], label) for label in labels)
                ),
                "exact_index_span_mrr_at_10": next(
                    (
                        1.0 / rank
                        for rank, item in enumerate(top, start=1)
                        if any(indexed_covers(item, label) for label in labels)
                    ),
                    0.0,
                ),
                "exact_index_span_recall_at_10": sum(
                    any(indexed_covers(item, label) for item in top) for label in labels
                )
                / len(labels),
                "scored_context_bytes_at_10": sum(
                    item["end_byte"] - item["start_byte"] for item in top
                ),
                "scored_context_tokens_at_10": sum(item["tokens"] for item in top),
                "indexed_bytes_at_10": sum(
                    item["span_accounting"]["indexed_end_byte"]
                    - item["span_accounting"]["indexed_start_byte"]
                    for item in top
                ),
                "extra_context_bytes_at_10": sum(
                    item["span_accounting"]["extra_context_bytes"] for item in top
                ),
            }
            rows.append({"task_id": task_id, "route": route, **metrics})
            for key, value in metrics.items():
                sums[key] += value
        routes[route] = {
            "status": "observed",
            "sample_count": len(answerable),
            "mean": {key: value / len(answerable) for key, value in sums.items()},
        }
    return {
        "scorer_identity": "rb-indexed-span-coverage-v1",
        "rank_source": "candidate_rank_v1",
        "scope": "diagnostic_not_primary_ndcg",
        "routes": routes,
        "per_query": rows,
        "per_candidate": sorted(
            candidate_rows, key=lambda row: (row["task_id"], row["route"], row["rank"])
        ),
    }


def ndcg_at_k(candidates: list[dict[str, Any]], labels: list[dict[str, Any]], k: int) -> float:
    """First-coverage graded NDCG, discounted by relevant byte density.

    A candidate earns only the highest newly covered grade. The union of its
    newly covered gold spans determines the useful fraction of its context.
    This makes an exact span ideal and prevents a whole-file chunk from
    receiving full relevance credit for a tiny embedded answer.
    """
    gains = []
    credited_labels: set[int] = set()
    for item in candidates[:k]:
        rel = 0
        new_spans: list[tuple[int, int]] = []
        for index, label in enumerate(labels):
            if index not in credited_labels and covers(item, label):
                rel = max(rel, int(label.get("grade", 1)))
                credited_labels.add(index)
                new_spans.append((int(label["start_byte"]), int(label["end_byte"])))
        span_bytes = int(item["end_byte"]) - int(item["start_byte"])
        require(span_bytes > 0, "rank candidate has an empty byte span")
        useful_bytes = 0
        cursor = int(item["start_byte"])
        for start, end in sorted(new_spans):
            useful_bytes += max(0, end - max(start, cursor))
            cursor = max(cursor, end)
        gains.append((2**rel - 1) * useful_bytes / span_bytes)
    dcg = sum(g / math.log2(i + 2) for i, g in enumerate(gains))
    ideal = sorted((int(label.get("grade", 1)) for label in labels), reverse=True)[:k]
    idcg = sum((2**g - 1) / math.log2(i + 2) for i, g in enumerate(ideal))
    if idcg == 0:
        return 0.0
    return dcg / idcg


def file_recall_at_k(
    candidates: list[dict[str, Any]], labels: list[dict[str, Any]], k: int
) -> float:
    gold_files = {label["path"] for label in labels}
    hit_files = gold_files.intersection({item["path"] for item in candidates[:k]})
    return len(hit_files) / len(gold_files)


def mean_ci(
    deltas: list[float],
    strata: list[tuple[str, str]] | None = None,
) -> dict[str, Any]:
    """Deterministic paired, within-stratum percentile bootstrap interval."""
    n = len(deltas)
    if strata is None:
        strata = [(str(index), "uncategorized") for index in range(n)]
    require(len(strata) == n, "confidence strata must align with paired deltas")
    paired = []
    for (task_id, category), delta in zip(strata, deltas):
        require(bool(task_id) and bool(category), "confidence strata identities must be nonempty")
        require(math.isfinite(delta), "confidence deltas must be finite")
        paired.append((task_id, category, delta))
    require(
        len({task_id for task_id, _category, _delta in paired}) == n,
        "confidence task identities must be unique",
    )
    paired.sort(key=lambda row: (row[0], row[1]))
    grouped: dict[str, list[tuple[str, float]]] = {}
    for task_id, category, delta in paired:
        grouped.setdefault(category, []).append((task_id, delta))
    stratum_counts = {key: len(grouped[key]) for key in sorted(grouped)}
    if n < MIN_CI_SAMPLE:
        return {
            "status": NOT_APPLICABLE,
            "reason": "insufficient_sample",
            "sample_count": n,
            "min_sample": MIN_CI_SAMPLE,
            "method": "paired_stratified_bootstrap_percentile_v1",
            "strata": stratum_counts,
        }
    mean = sum(delta for _task_id, _category, delta in paired) / n
    seed_material = [
        {"task_id": task_id, "stratum": category, "delta": delta}
        for task_id, category, delta in paired
    ]
    seed_bytes = canonical(seed_material)
    seed_sha256 = digest(seed_bytes)
    resamples = 10_000
    # Cache only pure numeric work, never repository/evidence admission. Exact
    # canonical bytes distinguish signed zero, identities, strata and values.
    # Large inputs bypass retention, bounding the cache to 32 * 64 KiB of keys.
    bounds = _bootstrap_bounds if len(seed_bytes) <= 65_536 else _bootstrap_bounds.__wrapped__
    lower, upper = bounds(seed_bytes, resamples)
    return {
        "sample_count": n,
        "method": "paired_stratified_bootstrap_percentile_v1",
        "resamples": resamples,
        "seed_sha256": seed_sha256,
        "strata": stratum_counts,
        "mean": mean,
        "lower_95": lower,
        "upper_95": upper,
    }


@lru_cache(maxsize=32)
def _bootstrap_bounds(seed_bytes: bytes, resamples: int) -> tuple[float, float]:
    """Immutable percentile values for an already validated canonical sample."""
    grouped: dict[str, list[float]] = {}
    for row in json.loads(seed_bytes):
        grouped.setdefault(row["stratum"], []).append(row["delta"])
    groups = [grouped[category] for category in sorted(grouped)]
    count = sum(len(rows) for rows in groups)
    # A constant stratum has only one possible resample. Keep the original
    # group/value addition order (including float rounding) for exact parity.
    if all(all(value == rows[0] for value in rows) for rows in groups):
        total = 0.0
        for rows in groups:
            for value in rows:
                total += value
        value = total / count
        # Keep percentile interpolation too: v*(1-w) + v*w can round
        # differently from v even when every sampled mean is identical.
        sampled_means = [value] * resamples
    else:
        rng = random.Random(int(digest(seed_bytes)[:16], 16))
        randrange = rng.randrange
        indexed_groups = [(rows, len(rows)) for rows in groups]
        sampled_means = []
        for _ in range(resamples):
            total = 0.0
            for rows, size in indexed_groups:
                for _index in range(size):
                    total += rows[randrange(size)]
            sampled_means.append(total / count)
    sampled_means.sort()

    def quantile(p: float) -> float:
        position = p * (len(sampled_means) - 1)
        lower = math.floor(position)
        upper = math.ceil(position)
        if lower == upper:
            return sampled_means[lower]
        weight = position - lower
        return sampled_means[lower] * (1.0 - weight) + sampled_means[upper] * weight

    return quantile(0.025), quantile(0.975)


def query_family_cluster_ci(
    rows: list[tuple[str, str, str, float]], repository_commit: str
) -> dict[str, Any]:
    """Qualification-only paired interval: resample whole query families."""
    require(bool(COMMIT_RE.fullmatch(repository_commit)), "cluster repository identity is invalid")
    seen: set[str] = set()
    families: dict[str, tuple[str, list[tuple[str, float]]]] = {}
    for task_id, family_id, category, delta in rows:
        require(
            bool(task_id) and bool(family_id) and bool(category),
            "cluster task, family and category identities must be nonempty",
        )
        require(task_id not in seen, "cluster task identities must be unique")
        require(math.isfinite(delta), "cluster deltas must be finite")
        seen.add(task_id)
        if family_id not in families:
            families[family_id] = (category, [])
        family_category, members = families[family_id]
        require(family_category == category, "query family crosses categories")
        members.append((task_id, delta))
    clusters = [
        (family_id, category, sorted(members))
        for family_id, (category, members) in sorted(families.items())
    ]
    strata: dict[str, list[tuple[float, int]]] = {}
    for _family_id, category, members in clusters:
        strata.setdefault(category, []).append(
            (sum(delta for _task_id, delta in members), len(members))
        )
    summary: dict[str, Any] = {
        "method": "paired_query_family_cluster_bootstrap_percentile_v1",
        "sample_count": len(rows),
        "cluster_count": len(clusters),
        "min_cluster_count": MIN_CI_SAMPLE,
        "strata": {category: len(group) for category, group in sorted(strata.items())},
    }
    if len(clusters) < MIN_CI_SAMPLE:
        return {
            **summary,
            "status": NOT_APPLICABLE,
            "reason": "insufficient_independent_clusters",
        }
    if MIN_CI_SAMPLE >= 20 and any(len(group) < 2 for group in strata.values()):
        return {
            **summary,
            "status": NOT_APPLICABLE,
            "reason": "insufficient_clusters_in_stratum",
        }
    seed = canonical(
        {
            "repository_commit": repository_commit,
            "clusters": [
                {"family_id": family_id, "category": category, "members": members}
                for family_id, category, members in clusters
            ],
        }
    )
    rng = random.Random(int(digest(seed)[:16], 16))
    sampled_means = []
    for _ in range(10_000):
        total = count = 0
        for category in sorted(strata):
            group = strata[category]
            for _cluster in group:
                cluster_sum, cluster_size = group[rng.randrange(len(group))]
                total += cluster_sum
                count += cluster_size
        sampled_means.append(total / count)
    sampled_means.sort()

    def quantile(p: float) -> float:
        position = p * (len(sampled_means) - 1)
        lower, upper = math.floor(position), math.ceil(position)
        weight = position - lower
        return sampled_means[lower] * (1.0 - weight) + sampled_means[upper] * weight

    return {
        **summary,
        "resamples": 10_000,
        "seed_sha256": digest(seed),
        "mean": sum(delta for _task_id, _family_id, _category, delta in rows) / len(rows),
        "lower_95": quantile(0.025),
        "upper_95": quantile(0.975),
    }


def qualified_query_family_ci(
    suite: dict[str, Any], report: dict[str, Any], baseline: str, candidate: str
) -> dict[str, Any]:
    """Derive independent-family uncertainty from the re-scored report rows."""
    primary = report["rank_metrics"]["comparison"]["primary_metric"]
    field = "ndcg_at_10" if primary == "ndcg_at_10" else "chunk_recall_at_10"
    require(primary in {"ndcg_at_10", "recall_at_10"}, "unsupported cluster primary metric")
    by_key = {(row["task_id"], row["route"]): row for row in report["per_query"]}
    tasks = sorted(
        (task for task in suite["tasks"] if task["split"] == "eval" and task["gold"]),
        key=lambda task: task["task_id"],
    )
    rows = []
    for task in tasks:
        task_id = task["task_id"]
        require(
            (task_id, baseline) in by_key and (task_id, candidate) in by_key,
            "cluster report is missing a paired task row",
        )
        before = by_key[(task_id, baseline)][field]
        after = by_key[(task_id, candidate)][field]
        require(
            type(before) in (float, int) and type(after) in (float, int),
            "cluster primary metric is not numeric",
        )
        rows.append(
            (
                task_id,
                task["query_family_id"],
                str(task.get("category", "uncategorized")),
                float(after - before),
            )
        )
    return query_family_cluster_ci(rows, suite["repository_commit"])


def _task_language(task: dict[str, Any]) -> str:
    suffixes = sorted(
        {
            (Path(label["path"]).suffix.lower().lstrip(".") or "extensionless")
            for label in task["gold"]
        }
    )
    if not suffixes:
        return "not_applicable:no_gold"
    return suffixes[0] if len(suffixes) == 1 else "mixed:" + "+".join(suffixes)


def stratified_delta_summary(
    rows: list[tuple[str, dict[str, Any], float]], repository_commit: str
) -> dict[str, Any]:
    dimensions = {
        "category": lambda task: str(task.get("category", "uncategorized")),
        "language": _task_language,
        "repository": lambda _task: repository_commit,
    }
    output: dict[str, Any] = {}
    for dimension, key_fn in dimensions.items():
        groups: dict[str, list[tuple[str, float]]] = {}
        for task_id, task, delta in rows:
            groups.setdefault(key_fn(task), []).append((task_id, delta))
        output[dimension] = {
            key: {
                "sample_count": len(group),
                "mean_delta": sum(delta for _task_id, delta in group) / len(group),
                "ci_95": mean_ci(
                    [delta for _task_id, delta in group],
                    [(task_id, key) for task_id, _delta in group],
                ),
            }
            for key, group in sorted(groups.items())
        }
    return output


def no_answer_delta_summary(
    rows: list[tuple[str, dict[str, Any], float]], repository_commit: str
) -> dict[str, Any]:
    deltas = [delta for _task_id, _task, delta in rows]
    return {
        "metric": "no_answer_abstention",
        "sample_count": len(rows),
        "mean_delta": (sum(deltas) / len(deltas)) if deltas else NOT_APPLICABLE,
        "ci_95": mean_ci(
            deltas,
            [
                (task_id, str(task.get("category", "uncategorized")))
                for task_id, task, _delta in rows
            ],
        ),
        "strata": stratified_delta_summary(rows, repository_commit),
    }


def _result_status(result: dict[str, Any]) -> str:
    return str(result["status"])


def _result_latency(result: dict[str, Any]) -> float | None:
    value = result["timings"]["query_latency_ms"]
    if value is None:
        return None
    return float(value)


def _result_error_code(result: dict[str, Any]) -> str | None:
    if result["error"] is None:
        return None
    return str(result["error"]["code"])


def _ordered_candidates(result: dict[str, Any]) -> list[dict[str, Any]]:
    candidates = list(result["candidates"])
    candidates.sort(key=lambda c: int(c["rank"]))
    return candidates


def evaluate(
    suite: dict[str, Any],
    pack: dict[str, Any],
    run: dict[str, Any],
    baseline: str,
    candidate: str,
    *,
    strict_k: bool = False,
) -> dict[str, Any]:
    # Historical v3 reports used capped @k values even when top_k < k. The
    # protocol-locked new report policy refuses that interpretation, while
    # legacy mode exists solely to reproduce immutable prior captures.
    if strict_k:
        require(
            suite["comparison_contract"]["top_k"] >= NDCG_K,
            "rank comparison requires top_k >= 10 for the declared @10 primary metric",
        )
    require(
        baseline in suite["routes"] and candidate in suite["routes"] and baseline != candidate,
        "comparison routes must be distinct registered routes",
    )
    eval_tasks = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
    task_ids = sorted(eval_tasks)
    routes = list(suite["routes"])
    results = {(row["task_id"], row["route"]): row for row in run["results"]}
    answerable_ids = [t for t in task_ids if eval_tasks[t]["gold"]]
    no_gold_ids = [t for t in task_ids if not eval_tasks[t]["gold"]]
    graded = bool(answerable_ids) and all(
        all("grade" in label for label in eval_tasks[t]["gold"]) for t in answerable_ids
    )
    primary_metric = "ndcg_at_10" if graded else "recall_at_10"
    declared_top_k = int(suite["comparison_contract"]["top_k"])
    ordered = sorted(suite["file_universe"], key=lambda e: str(e["path"]))
    file_universe_digest = digest(canonical(ordered))
    output: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "suite_id": suite["suite_id"],
        "suite_commitment_sha256": digest(canonical(suite)),
        "query_pack_sha256": digest(canonical(pack)),
        "runner_record_sha256": digest(canonical(run)),
        "repository_commit": suite["repository_commit"],
        "runner": run["runner"],
        "tokenizer": TOKENIZER,
        "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
        "file_universe_digest": file_universe_digest,
        "primary_metric": primary_metric,
        "graded": graded,
        "answerable_tasks": len(answerable_ids),
        "no_gold_tasks": len(no_gold_ids),
        "sample_count": len(task_ids),
        "budgets": {},
    }
    output["rank_metric_version"] = "rb-rank-context-density-first-coverage"
    output["route_provenance"] = run["route_provenance"]
    output["blinding"] = run["runner"]["blinding"]
    output["comparison_contract"] = run["comparison_contract"]
    output["captures"] = run["captures"]
    # Budgeted BCY view.
    for budget in BUDGETS:
        per_route: dict[str, Any] = {}
        successes: dict[str, dict[str, bool]] = {}
        for route in routes:
            answerable = 0
            no_gold = 0
            full = 0
            file_recall = 0.0
            block_recall = 0.0
            abstentions = 0
            consumed = 0
            latencies: list[float] = []
            status_counts: dict[str, int] = {}
            outcomes: dict[str, bool] = {}
            for task_id in task_ids:
                task = eval_tasks[task_id]
                result = results[(task_id, route)]
                status = _result_status(result)
                status_counts[status] = status_counts.get(status, 0) + 1
                latency = _result_latency(result)
                if latency is not None:
                    latencies.append(latency)
                candidates = _ordered_candidates(result)
                chosen, used = selected(candidates, budget)
                consumed += used
                labels = task["gold"]
                if not labels:
                    no_gold += 1
                    success = status == "abstained"
                    abstentions += int(success)
                else:
                    answerable += 1
                    if status in SCORED_STATUSES:
                        gold_files = {label["path"] for label in labels}
                        hit_files = gold_files.intersection({item["path"] for item in chosen})
                        file_recall += len(hit_files) / len(gold_files)
                        hit_blocks = sum(
                            any(covers(item, label) for item in chosen) for label in labels
                        )
                        block_recall += hit_blocks / len(labels)
                        success = hit_blocks == len(labels)
                    else:
                        success = False
                    full += int(success)
                outcomes[task_id] = success
            entry: dict[str, Any] = {
                "answerable_tasks": answerable,
                "no_gold_tasks": no_gold,
                "bcy": (full / answerable) if answerable else NOT_APPLICABLE,
                "file_recall": (file_recall / answerable) if answerable else NOT_APPLICABLE,
                "block_recall": (block_recall / answerable) if answerable else NOT_APPLICABLE,
                "no_gold_abstention": (abstentions / no_gold) if no_gold else NOT_APPLICABLE,
                "mean_context_tokens": consumed / len(task_ids),
                "mean_query_latency_ms": (
                    (sum(latencies) / len(latencies)) if latencies else NOT_APPLICABLE
                ),
                "status_counts": status_counts,
                "sample_count": len(task_ids),
            }
            per_route[route] = entry
            successes[route] = outcomes
        comparisons = {}
        for metric in (
            "bcy",
            "file_recall",
            "block_recall",
            "no_gold_abstention",
            "mean_context_tokens",
        ):
            a = per_route[baseline][metric]
            b = per_route[candidate][metric]
            if a == NOT_APPLICABLE or b == NOT_APPLICABLE:
                comparisons[metric] = NOT_APPLICABLE
            else:
                comparisons[metric] = b - a
        wins = sum(successes[candidate][t] and not successes[baseline][t] for t in task_ids)
        losses = sum(successes[baseline][t] and not successes[candidate][t] for t in task_ids)
        binary_deltas = [
            float(successes[candidate][t]) - float(successes[baseline][t]) for t in task_ids
        ]
        output["budgets"][str(budget)] = {
            "routes": per_route,
            "comparison": {
                "baseline": baseline,
                "candidate": candidate,
                "delta": comparisons,
                "paired_wins": wins,
                "paired_losses": losses,
                "paired_ties": len(task_ids) - wins - losses,
                "sample_count": len(task_ids),
                "success_delta_ci_95": mean_ci(
                    binary_deltas,
                    [
                        (task_id, str(eval_tasks[task_id].get("category", "uncategorized")))
                        for task_id in task_ids
                    ],
                ),
            },
        }
    # Rank-based quality view with chunk-level and same-file-collapsed behavior.
    rank_routes: dict[str, Any] = {}
    per_task_primary: dict[str, dict[str, float]] = {t: {} for t in answerable_ids}
    for route in routes:
        chunk_sums: dict[str, float] = {f"recall_at_{k}": 0.0 for k in RECALL_KS}
        chunk_sums["mrr_at_10"] = 0.0
        chunk_sums["ndcg_at_10"] = 0.0
        chunk_sums["file_recall_at_10"] = 0.0
        collapsed_sums = dict(chunk_sums)
        latencies: list[float] = []
        status_counts: dict[str, int] = {}
        for task_id in task_ids:
            result = results[(task_id, route)]
            status = _result_status(result)
            status_counts[status] = status_counts.get(status, 0) + 1
            latency = _result_latency(result)
            if latency is not None:
                latencies.append(latency)
            if task_id not in per_task_primary:
                continue
            labels = eval_tasks[task_id]["gold"]
            if status in SCORED_STATUSES:
                candidates = _ordered_candidates(result)
                collapsed = collapse_by_file(candidates)
                chunk_vals = {
                    f"recall_at_{k}": recall_at_k(candidates, labels, k) for k in RECALL_KS
                }
                chunk_vals["mrr_at_10"] = mrr_at_k(candidates, labels, MRR_K)
                chunk_vals["ndcg_at_10"] = ndcg_at_k(candidates, labels, NDCG_K) if graded else 0.0
                chunk_vals["file_recall_at_10"] = file_recall_at_k(candidates, labels, MRR_K)
                collapsed_vals = {
                    f"recall_at_{k}": recall_at_k(collapsed, labels, k) for k in RECALL_KS
                }
                collapsed_vals["mrr_at_10"] = mrr_at_k(collapsed, labels, MRR_K)
                collapsed_vals["ndcg_at_10"] = (
                    ndcg_at_k(collapsed, labels, NDCG_K) if graded else 0.0
                )
                collapsed_vals["file_recall_at_10"] = file_recall_at_k(collapsed, labels, MRR_K)
            else:
                chunk_vals = {k: 0.0 for k in chunk_sums}
                collapsed_vals = {k: 0.0 for k in collapsed_sums}
            for key, value in chunk_vals.items():
                chunk_sums[key] += value
            for key, value in collapsed_vals.items():
                collapsed_sums[key] += value
            per_task_primary[task_id][route] = chunk_vals[primary_metric]
        n = len(answerable_ids)

        def _avg(sums: dict[str, float], sample_count: int = n) -> dict[str, Any]:
            if not sample_count:
                return {k: NOT_APPLICABLE for k in sums}
            averaged: dict[str, Any] = {k: v / sample_count for k, v in sums.items()}
            if strict_k:
                for k in RECALL_KS:
                    if k > declared_top_k:
                        averaged[f"recall_at_{k}"] = NOT_APPLICABLE
            if not graded:
                averaged["ndcg_at_10"] = NOT_APPLICABLE
            return averaged

        rank_routes[route] = {
            "answerable_tasks": len(answerable_ids),
            "no_gold_tasks": len(no_gold_ids),
            "chunk": _avg(chunk_sums),
            "collapsed": _avg(collapsed_sums),
            "mean_query_latency_ms": (
                (sum(latencies) / len(latencies)) if latencies else NOT_APPLICABLE
            ),
            "status_counts": status_counts,
            "sample_count": len(task_ids),
        }
    no_answer_rows = [
        (
            task_id,
            eval_tasks[task_id],
            float(_result_status(results[(task_id, candidate)]) == "abstained")
            - float(_result_status(results[(task_id, baseline)]) == "abstained"),
        )
        for task_id in no_gold_ids
    ]
    no_answer_evidence = no_answer_delta_summary(no_answer_rows, suite["repository_commit"])
    if answerable_ids:
        primary_deltas = [
            per_task_primary[t][candidate] - per_task_primary[t][baseline] for t in answerable_ids
        ]
        rank_wins = sum(d > 0 for d in primary_deltas)
        rank_losses = sum(d < 0 for d in primary_deltas)
        base_chunk = rank_routes[baseline]["chunk"]
        cand_chunk = rank_routes[candidate]["chunk"]
        rank_delta = {}
        for key in base_chunk:
            a = base_chunk[key]
            b = cand_chunk[key]
            rank_delta[key] = (
                NOT_APPLICABLE if (a == NOT_APPLICABLE or b == NOT_APPLICABLE) else (b - a)
            )
        rank_comparison: dict[str, Any] = {
            "baseline": baseline,
            "candidate": candidate,
            "primary_metric": primary_metric,
            "delta": rank_delta,
            "primary_delta": rank_delta[primary_metric],
            "paired_wins": rank_wins,
            "paired_losses": rank_losses,
            "paired_ties": len(answerable_ids) - rank_wins - rank_losses,
            "sample_count": len(answerable_ids),
            "primary_delta_ci_95": mean_ci(
                primary_deltas,
                [
                    (task_id, str(eval_tasks[task_id].get("category", "uncategorized")))
                    for task_id in answerable_ids
                ],
            ),
            "stratified_primary_delta": stratified_delta_summary(
                [
                    (task_id, eval_tasks[task_id], delta)
                    for task_id, delta in zip(answerable_ids, primary_deltas)
                ],
                suite["repository_commit"],
            ),
            "no_answer_abstention_delta": no_answer_evidence,
        }
    else:
        rank_comparison = {
            "baseline": baseline,
            "candidate": candidate,
            "primary_metric": primary_metric,
            "delta": NOT_APPLICABLE,
            "primary_delta": NOT_APPLICABLE,
            "paired_wins": 0,
            "paired_losses": 0,
            "paired_ties": 0,
            "sample_count": 0,
            "primary_delta_ci_95": mean_ci([]),
            "stratified_primary_delta": {
                "category": {},
                "language": {},
                "repository": {},
            },
            "no_answer_abstention_delta": no_answer_evidence,
        }
    output["rank_metrics"] = {"routes": rank_routes, "comparison": rank_comparison}
    span_accounting = indexed_span_diagnostics(run, results, eval_tasks)
    if span_accounting is not None:
        output["span_accounting"] = span_accounting
    # Per-query rows in deterministic task/route order.
    rows = []
    for task_id in task_ids:
        task = eval_tasks[task_id]
        labels = task["gold"]
        for route in sorted(routes):
            result = results[(task_id, route)]
            status = _result_status(result)
            candidates = _ordered_candidates(result)
            collapsed = collapse_by_file(candidates)
            if not labels:
                row = {
                    "task_id": task_id,
                    "route": route,
                    "answerable": False,
                    "category": task.get("category"),
                    "status": status,
                    "candidates": len(candidates),
                    "chunk_recall_at_10": NOT_APPLICABLE,
                    "collapsed_recall_at_10": NOT_APPLICABLE,
                    "mrr_at_10": NOT_APPLICABLE,
                    "ndcg_at_10": NOT_APPLICABLE,
                    "file_hit_at_10": NOT_APPLICABLE,
                    "file_recall_at_10": NOT_APPLICABLE,
                    "error_code": _result_error_code(result),
                    "query_latency_ms": _result_latency(result),
                }
            elif status in SCORED_STATUSES:
                row = {
                    "task_id": task_id,
                    "route": route,
                    "answerable": True,
                    "category": task.get("category"),
                    "status": status,
                    "candidates": len(candidates),
                    "chunk_recall_at_10": recall_at_k(candidates, labels, 10),
                    "collapsed_recall_at_10": recall_at_k(collapsed, labels, 10),
                    "mrr_at_10": mrr_at_k(candidates, labels, MRR_K),
                    "ndcg_at_10": (
                        ndcg_at_k(candidates, labels, NDCG_K) if graded else NOT_APPLICABLE
                    ),
                    "file_hit_at_10": bool(file_recall_at_k(candidates, labels, MRR_K) > 0),
                    "file_recall_at_10": file_recall_at_k(candidates, labels, MRR_K),
                    "error_code": None,
                    "query_latency_ms": _result_latency(result),
                }
            else:
                row = {
                    "task_id": task_id,
                    "route": route,
                    "answerable": True,
                    "category": task.get("category"),
                    "status": status,
                    "candidates": 0,
                    "chunk_recall_at_10": 0.0,
                    "collapsed_recall_at_10": 0.0,
                    "mrr_at_10": 0.0,
                    "ndcg_at_10": (0.0 if graded else NOT_APPLICABLE),
                    "file_hit_at_10": False,
                    "file_recall_at_10": 0.0,
                    "error_code": _result_error_code(result),
                    "query_latency_ms": _result_latency(result),
                }
            rows.append(row)
    output["per_query"] = rows
    return output


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    for command in ("freeze", "evaluate"):
        child = sub.add_parser(command)
        child.add_argument("--repo", type=Path, required=True)
        child.add_argument("--suite", type=Path, required=True)
        child.add_argument("--output", type=Path)
        if command == "evaluate":
            child.add_argument("--runner", type=Path, required=True)
            child.add_argument("--baseline-route", required=True)
            child.add_argument("--candidate-route", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "freeze":
            _, value, _ = validate_suite(args.repo.resolve(), read_json(args.suite))
        else:
            suite, pack, run = load_evidence(args.repo.resolve(), args.suite, args.runner)
            value = evaluate(
                suite, pack, run, args.baseline_route, args.candidate_route, strict_k=True
            )
        rendered = (
            json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False, allow_nan=False) + "\n"
        )
        if args.output:
            args.output.write_text(rendered, encoding="utf-8")
            if args.command == "freeze":
                sys.stdout.write(digest(canonical(value)) + "\n")
        else:
            sys.stdout.write(rendered)
        return 0
    except (EvidenceError, OSError) as exc:
        print("ERROR: " + str(exc), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
