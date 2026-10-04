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
import os
import random
import re
import subprocess
import sys
from array import array
from bisect import bisect_right
from collections.abc import Sequence
from functools import lru_cache
from pathlib import Path, PurePosixPath
from typing import Any, BinaryIO

try:
    from tools.benchmark.evidence import (
        EvidenceError as SourceReadError,
    )
    from tools.benchmark.evidence import (
        _consume_regular_file,
    )
    from tools.benchmark.retrieval import literal_source_oracle, retrieval_contract, source_oracle
    from tools.benchmark.retrieval import query_plan as query_plan_contract
    from tools.benchmark.retrieval.finite_json import is_finite_json_number
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.evidence import (
        EvidenceError as SourceReadError,
    )
    from tools.benchmark.evidence import (
        _consume_regular_file,
    )
    from tools.benchmark.retrieval import literal_source_oracle, retrieval_contract, source_oracle
    from tools.benchmark.retrieval import query_plan as query_plan_contract
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
QUERY_INTENTS = ("bare_symbol", "symbol_components", "semantic_intent", "exact_content")
LABEL_REVIEW_ASSESSMENTS = ("unreviewed", "reviewed_unambiguous", "reviewed_ambiguous")
OBSERVED_PREFIX_DIAGNOSTIC_POLICY = "observed_prefix_v1"
UNJUDGED_POLICY = "unjudged_zero_v1"
COMPLETE_JUDGMENT_POLICY = "complete_ranked_pool_v1"
SOURCE_ORACLE_JUDGMENT_POLICY = "source_oracle_complete_v1"
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


def judgment_grade(value: Any, where: str) -> int:
    require(type(value) is int and 0 <= value <= 3, f"{where} grade must be an integer 0-3")
    return value


def answerability_min_grade(task: dict[str, Any], where: str) -> int:
    """Keep sufficient-answer grades separate from partial relevance gains."""
    value = task.get("answerability_min_grade", 1)
    require(
        type(value) is int and 1 <= value <= 3,
        f"{where} answerability_min_grade must be an integer 1-3",
    )
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


def validate_evaluation_contract(task: dict[str, Any], task_id: str) -> dict[str, str]:
    """Bind a task's requested search behavior to its judged and returned unit."""
    contract = object_keys(
        task["evaluation_contract"],
        ["request_mode", "gold_unit", "result_unit"],
        f"evaluation_contract for {task_id}",
    )
    mode = contract["request_mode"]
    require(
        mode in query_plan_contract.EVALUATION_REQUEST_MODES,
        f"unknown request_mode for {task_id}",
    )
    unit = "symbol" if mode == query_plan_contract.DECLARATION_NAVIGATION else "distinct_file"
    require(
        contract["gold_unit"] == unit and contract["result_unit"] == unit,
        f"evaluation_contract unit mismatch for {task_id}",
    )
    kind = "declaration_judgments" if unit == "symbol" else "file_judgments"
    opposite = "file_judgments" if unit == "symbol" else "declaration_judgments"
    require(
        kind in task and opposite not in task,
        f"evaluation_contract requires {kind} only for {task_id}",
    )
    if "source_oracle" in task:
        require(
            task["source_oracle"].get("unit") == unit,
            f"evaluation_contract/source_oracle unit mismatch for {task_id}",
        )
        if mode == query_plan_contract.DECLARATION_NAVIGATION:
            require(
                source_oracle.NAME_CONTRACTS.get(
                    task["source_oracle"].get("contract"), (None, None)
                )[1]
                == "exact",
                f"declaration_navigation requires exact-name source oracle for {task_id}",
            )
    if mode in (
        query_plan_contract.EXPLICIT_OSA1_TYPO,
        query_plan_contract.DECLARATION_NAVIGATION,
    ):
        require(
            task.get("query_intent") == "bare_symbol",
            f"evaluation_contract requires bare_symbol intent for {task_id}",
        )
    if mode == query_plan_contract.EXPLICIT_SYMBOL_COMPONENTS:
        require(
            task.get("query_intent") == "symbol_components",
            f"explicit_symbol_components requires symbol_components intent for {task_id}",
        )
    if mode == query_plan_contract.NATURAL_LANGUAGE_FILE_SEARCH:
        require(
            task.get("query_intent") == "semantic_intent" and "source_oracle" not in task,
            f"natural_language_file_search requires independently judged semantic_intent for {task_id}",
        )
    return contract


def declared_evaluation_contract(tasks: Sequence[dict[str, Any]]) -> dict[str, str] | None:
    """One report/route scores one request mode; missing metadata means legacy replay."""
    present = [task for task in tasks if "evaluation_contract" in task]
    if not present:
        return None
    require(len(present) == len(tasks), "partial evaluation_contract coverage")
    contracts = [validate_evaluation_contract(task, task["task_id"]) for task in tasks]
    require(
        all(contract == contracts[0] for contract in contracts),
        "mixed request modes or ranking units in one suite",
    )
    return contracts[0]


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
        try:
            expected = query_plan_contract.execution_profile(
                policy,
                config if policy in ("natural_language", "natural_language_file") else None,
            )
        except query_plan_contract.QueryPlanError as exc:
            raise EvidenceError(f"{where}.config is invalid: {exc}") from exc
        require(profile == expected, f"{where} differs from the frozen Quanta profile")
        return profile
    profile = object_keys(value, ["profile_id", "mode", "alpha", "rerank"], where)
    modes = {
        "native-default": ("semble-native-default-v1", None, "upstream-content-default"),
        "lexical-only": ("semble-lexical-only-v1", None, "not_applicable"),
        "lexical-file": ("semble-lexical-file-v1", None, "not_applicable"),
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
    exact_content = (
        version == 5
        and isinstance(value, dict)
        and isinstance(value.get("execution_profile"), dict)
        and value["execution_profile"].get("policy") == "code_search_exact_content_file"
    )
    source_bound = exact_content or (
        version == 5
        and isinstance(value, dict)
        and isinstance(value.get("execution_profile"), dict)
        and value["execution_profile"].get("policy") == "code_search_file"
        and ("source_repo_id" in value or "source_revision_id" in value)
    )
    if source_bound:
        fields.extend(["source_repo_id", "source_revision_id"])
    capture = object_keys(
        value,
        fields,
        where,
    )
    system = capture["system"]
    require(system in CAPTURE_SYSTEMS, f"{where}.system must be quanta or semble")
    if source_bound:
        require(system == "quanta", f"{where}.source-bound capture must be Quanta")
        string(capture["source_repo_id"], where + ".source_repo_id")
        string(capture["source_revision_id"], where + ".source_revision_id")
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

    def __init__(self, repo: Path, commit: str, *, max_total_bytes: int | None = None) -> None:
        self.repo = verify_repo(repo, commit)
        self.commit = commit
        self.max_total_bytes = max_total_bytes
        self.cached_source_bytes = 0
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
        self.line_starts: dict[str, array] = {}

    def file(self, name: str) -> tuple[bytes, list[bytes], str]:
        if name not in self.files:
            absolute = safe_path(self, name)
            if self.max_total_bytes is None:
                raw = absolute.read_bytes()
            else:
                remaining = self.max_total_bytes - self.cached_source_bytes

                def read_bounded(handle: BinaryIO) -> bytes:
                    if os.fstat(handle.fileno()).st_size > remaining:
                        raise EvidenceError("source oracle byte limit exceeded")
                    data = handle.read(remaining + 1)
                    require(len(data) <= remaining, "source oracle byte limit exceeded")
                    return data

                try:
                    raw = _consume_regular_file(absolute, read_bounded)
                except SourceReadError as exc:
                    raise EvidenceError(f"source oracle source unavailable: {exc}") from exc
                self.cached_source_bytes += len(raw)
            self.files[name] = (raw, raw.splitlines(keepends=True), digest(raw))
        return self.files[name]

    def source_line_at(self, name: str, byte_offset: int) -> tuple[int, int, bytes]:
        """Resolve a byte offset against splitlines boundaries indexed once per file."""
        raw, lines, _digest = self.file(name)
        starts = self.line_starts.get(name)
        if starts is None:
            starts = array("Q")
            offset = 0
            for contents in lines:
                starts.append(offset)
                offset += len(contents)
            self.line_starts[name] = starts
        line_index = bisect_right(starts, byte_offset) - 1
        require(
            0 <= line_index < len(lines) and byte_offset < len(raw),
            "source oracle match has no source line",
        )
        return line_index + 1, starts[line_index], lines[line_index]


def source_oracle_gold(
    source: SourceSnapshot,
    oracle: source_oracle.SourceOracleIndex | literal_source_oracle.LiteralSourceOracleIndex,
    contract: str,
    query: str,
) -> list[dict[str, Any]]:
    """Project the oracle's first source match onto its complete source line."""
    match = oracle.first_match(contract, query)
    if match is None:
        return []
    path, start, end = match
    raw, _lines, file_sha256 = source.file(path)
    require(file_sha256 == oracle.files[path][1], "source oracle file hash drift")
    line_number, line_start, contents = source.source_line_at(path, start)
    line_end = line_start + len(contents)
    require(end <= line_end, "source oracle match crosses a source line")
    return [
        {
            "path": path,
            "start_byte": line_start,
            "end_byte": line_end,
            "start_line": line_number,
            "end_line": line_number,
            "file_sha256": file_sha256,
            "block_sha256": digest(raw[line_start:line_end]),
            "grade": 3,
        }
    ]


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


def _file_candidate_block(
    source: SourceSnapshot,
    value: Any,
    where: str,
    universe: set[str] | None,
    allow_score: bool,
) -> dict[str, Any]:
    """Replay the file unit from frozen bytes, including an empty path hit."""
    required = [
        "path",
        "start_line",
        "end_line",
        "start_byte",
        "end_byte",
        "file_sha256",
        "block_sha256",
        "tokens",
        "rank",
        "span_accounting",
    ]
    item = object_keys_optional(value, required, ["score"] if allow_score else [], where)
    path = string(item["path"], where + ".path")
    raw, lines, file_digest = source.file(path)
    if universe is not None:
        require(path in universe, where + f" file excluded from file universe: {path}")
    require(
        sha(item["file_sha256"], where + ".file_sha256") == file_digest,
        where + " file hash mismatch",
    )
    require(
        sha(item["block_sha256"], where + ".block_sha256") == file_digest,
        where + " full-file block hash mismatch",
    )
    require(
        nonnegative_int(item["start_byte"], where + ".start_byte") == 0,
        where + " file unit must begin at byte zero",
    )
    require(
        nonnegative_int(item["end_byte"], where + ".end_byte") == len(raw),
        where + " file unit must cover entire source",
    )
    expected_lines = (0, 0) if not lines else (1, len(lines))
    require(
        (item["start_line"], item["end_line"]) == expected_lines,
        where + " file line projection differs from source",
    )
    positive_int(item["rank"], where + ".rank")
    if "score" in item:
        require(is_finite_json_number(item["score"]), where + ".score must be finite")

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
            "source_repo_id",
            "source_revision_id",
            "preview_kind",
            "preview_start_byte",
            "preview_end_byte",
            "snippet_sha256",
        ],
        where + ".span_accounting",
    )
    require(accounting["unit_kind"] == "file", where + " has wrong file unit kind")
    require(
        accounting["producer_identity"] == "code-search-file-v1",
        where + " has wrong file producer identity",
    )
    repo_id = string(accounting["source_repo_id"], where + ".source_repo_id")
    string(accounting["source_revision_id"], where + ".source_revision_id")
    repo = repo_id.encode("utf-8")
    path_bytes = path.encode("utf-8")
    framed = (
        b"quanta-index:code-search-file:v1\x00"
        + len(repo).to_bytes(8, "little")
        + repo
        + len(path_bytes).to_bytes(8, "little")
        + path_bytes
    )
    require(
        accounting["unit_id"] == "file:" + digest(framed), where + " file identity digest mismatch"
    )
    require(
        nonnegative_int(accounting["indexed_start_byte"], where + ".indexed_start_byte") == 0
        and nonnegative_int(accounting["indexed_end_byte"], where + ".indexed_end_byte")
        == len(raw),
        where + " indexed file span differs from source",
    )
    require(
        nonnegative_int(accounting["extra_context_bytes"], where + ".extra_context_bytes") == 0,
        where + " file context expansion must be zero",
    )
    kind = accounting["preview_kind"]
    if kind == "source_file":
        preview_start = nonnegative_int(
            accounting["preview_start_byte"], where + ".preview_start_byte"
        )
        preview_end = nonnegative_int(accounting["preview_end_byte"], where + ".preview_end_byte")
        require(preview_start < preview_end <= len(raw), where + " source preview span is invalid")
        snippet = raw[preview_start:preview_end]
    elif kind == "path":
        require(
            accounting["preview_start_byte"] is None and accounting["preview_end_byte"] is None,
            where + " path preview cannot claim source bytes",
        )
        snippet = path_bytes
    elif kind == "source_file_unavailable":
        require(
            accounting["preview_start_byte"] is None and accounting["preview_end_byte"] is None,
            where + " unavailable preview cannot claim source bytes",
        )
        snippet = b""
    else:
        raise EvidenceError(where + " has unknown file preview kind")
    require(
        sha(accounting["snippet_sha256"], where + ".snippet_sha256") == digest(snippet),
        where + " snippet differs from frozen source/path",
    )
    try:
        preview_text = snippet.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise EvidenceError(where + " preview cuts a UTF-8 boundary") from exc
    require(
        nonnegative_int(item["tokens"], where + ".tokens") == len(TOKEN_RE.findall(preview_text)),
        where + " preview token count mismatch",
    )
    sdk_start = nonnegative_int(accounting["sdk_start_line"], where + ".sdk_start_line")
    sdk_end = nonnegative_int(accounting["sdk_end_line"], where + ".sdk_end_line")
    require(
        (sdk_start == sdk_end == 0) or (1 <= sdk_start <= sdk_end <= len(lines)),
        where + " SDK file line span is outside source",
    )
    return item


def validate_name_span(
    value: Any, raw: bytes, definition_start: int, definition_end: int, where: str
) -> dict[str, Any]:
    """Validate explicit name bytes inside a definition; never infer from context."""
    span = object_keys(value, ["start_byte", "end_byte", "name"], where)
    start = nonnegative_int(span["start_byte"], where + ".start_byte")
    end = positive_int(span["end_byte"], where + ".end_byte")
    name = string(span["name"], where + ".name")
    require(
        definition_start <= start < end <= definition_end,
        where + " name span escapes its definition",
    )
    try:
        actual = raw[start:end].decode("utf-8")
    except UnicodeDecodeError as exc:
        raise EvidenceError(where + " name span cuts a UTF-8 boundary") from exc
    require(actual == name, where + " name differs from source bytes")
    return span


def block(
    source: SourceSnapshot,
    value: Any,
    where: str,
    *,
    candidate: bool,
    universe: set[str] | None = None,
    allow_grade: bool = False,
    allow_span_accounting: bool = False,
    allow_score: bool = False,
) -> dict[str, Any]:
    if (
        candidate
        and allow_span_accounting
        and isinstance(value, dict)
        and isinstance(value.get("span_accounting"), dict)
        and value["span_accounting"].get("unit_kind") == "file"
    ):
        return _file_candidate_block(source, value, where, universe, allow_score)
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
    if candidate and allow_score:
        optional.append("score")
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
        if "score" in item:
            require(is_finite_json_number(item["score"]), where + ".score must be finite")
        if "span_accounting" in item:
            accounting = object_keys_optional(
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
                ["name_span"],
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
            if "name_span" in accounting:
                require(
                    accounting["unit_kind"] == "symbol", where + " name span requires symbol unit"
                )
                validate_name_span(
                    accounting["name_span"], raw, indexed_start, indexed_end, where + ".name_span"
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


def validate_judgments(
    source: SourceSnapshot,
    task: dict[str, Any],
    universe: set[str],
    task_id: str,
) -> None:
    """Bind optional independent file and declaration judgments to source bytes."""
    kinds = ("file_judgments", "declaration_judgments")
    present = [kind for kind in kinds if kind in task]
    if "answerability_min_grade" in task:
        require(
            bool(present) and "source_oracle" not in task,
            f"answerability_min_grade requires independent judgments: {task_id}",
        )
    answer_grade = answerability_min_grade(task, task_id)
    if not present:
        require("judgment_policy" not in task, f"orphan judgment_policy: {task_id}")
        require("source_oracle" not in task, f"source oracle lacks judgments: {task_id}")
        return
    if "source_oracle" in task:
        require(
            task.get("judgment_policy") == SOURCE_ORACLE_JUDGMENT_POLICY,
            f"source oracle requires {SOURCE_ORACLE_JUDGMENT_POLICY}: {task_id}",
        )
        require("label_review" not in task, f"source oracle cannot claim human review: {task_id}")
    else:
        require(
            task.get("judgment_policy") in (UNJUDGED_POLICY, COMPLETE_JUDGMENT_POLICY),
            f"judgments require explicit {UNJUDGED_POLICY} or "
            f"{COMPLETE_JUDGMENT_POLICY} policy: {task_id}",
        )
        review = task.get("label_review")
        require(
            isinstance(review, dict) and review.get("assessment") in LABEL_REVIEW_ASSESSMENTS[1:],
            f"independent judgments require reviewed label evidence: {task_id}",
        )
    for kind in present:
        judgments = task[kind]
        require(isinstance(judgments, list), f"{kind} must be a list: {task_id}")
        seen: set[tuple[Any, ...]] = set()
        for value in judgments:
            where = f"{kind} for {task_id}"
            fields = ["path", "file_sha256", "grade"]
            if kind == "declaration_judgments":
                fields.extend(["start_byte", "end_byte"])
            item = (
                object_keys_optional(value, fields, ["name_span"], where)
                if kind == "declaration_judgments"
                else object_keys(value, fields, where)
            )
            path = string(item["path"], where + ".path")
            require(path in universe, f"{where} file excluded from file universe: {path}")
            raw, _lines, file_digest = source.file(path)
            require(
                sha(item["file_sha256"], where + ".file_sha256") == file_digest,
                f"{where} file hash mismatch: {path}",
            )
            judgment_grade(item["grade"], where)
            key: tuple[Any, ...] = (path,)
            if kind == "declaration_judgments":
                start = nonnegative_int(item["start_byte"], where + ".start_byte")
                end = positive_int(item["end_byte"], where + ".end_byte")
                require(start < end <= len(raw), f"{where} has invalid source byte span")
                try:
                    selected = raw[start:end].decode("utf-8")
                except UnicodeDecodeError as exc:
                    raise EvidenceError(f"{where} cuts a UTF-8 boundary") from exc
                require(bool(TOKEN_RE.search(selected)), f"{where} has no retrievable token")
                if "name_span" in item:
                    validate_name_span(item["name_span"], raw, start, end, where + ".name_span")
                key = (path, start, end)
            require(key not in seen, f"duplicate {kind}: {task_id} {key}")
            seen.add(key)
        if task["answerable"]:
            require(
                any(row["grade"] >= answer_grade for row in judgments),
                f"{kind} lacks a positive judgment at answerability_min_grade: {task_id}",
            )
        else:
            require(
                not any(row["grade"] >= answer_grade for row in judgments),
                f"{kind} answerability mismatch: {task_id}",
            )


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
    repo: Path,
    payload: Any,
    *,
    source_oracle_admission: bool = False,
    declaration_census_cache: source_oracle.DeclarationCensusCache | None = None,
    source_snapshot: SourceSnapshot | None = None,
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
        ["leakage_allowlist", "diagnostic_policy"],
        "suite",
    )
    require(
        type(suite["schema_version"]) is int and suite["schema_version"] == SCHEMA_VERSION,
        "unsupported suite schema",
    )
    contract = validate_comparison_contract(
        suite["comparison_contract"], "suite.comparison_contract"
    )
    if "diagnostic_policy" in suite:
        require(
            suite["diagnostic_policy"] == OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
            "unsupported suite diagnostic_policy",
        )
        require(contract["top_k"] >= MRR_K, "observed-prefix diagnostics require top_k >= 10")
    string(suite["suite_id"], "suite_id")
    commit = suite["repository_commit"]
    require(
        isinstance(commit, str) and bool(COMMIT_RE.fullmatch(commit)),
        "repository_commit must be a full lowercase Git SHA",
    )
    tasks = suite["tasks"]
    # The suite builder opts in before adding source_oracle annotations to its baseline.
    annotated_oracle_tasks = (
        [task for task in tasks if isinstance(task, dict) and "source_oracle" in task]
        if isinstance(tasks, list)
        else []
    )
    if source_oracle_admission or annotated_oracle_tasks:
        require(
            isinstance(suite["file_universe"], list)
            and 0 < len(suite["file_universe"]) <= source_oracle.MAX_FILES,
            "source oracle file limit exceeded",
        )
        require(
            isinstance(tasks, list)
            and 0
            < (len(tasks) if source_oracle_admission else len(annotated_oracle_tasks))
            <= source_oracle.MAX_QUERIES,
            "source oracle query limit exceeded",
        )
    source_byte_limit = (
        source_oracle.MAX_SOURCE_BYTES
        if source_oracle_admission or annotated_oracle_tasks
        else None
    )
    if source_snapshot is None:
        source = SourceSnapshot(repo, commit, max_total_bytes=source_byte_limit)
    else:
        resolved_repo = verify_repo(repo, commit)
        require(
            source_snapshot.repo == resolved_repo
            and source_snapshot.commit == commit
            and source_snapshot.max_total_bytes == source_byte_limit,
            "shared source snapshot repository or byte limit differs",
        )
        source = source_snapshot
    routes = suite["routes"]
    require(isinstance(routes, list) and len(routes) >= 1, "suite requires at least one route")
    for route in routes:
        string(route, "route")
    require(len(set(routes)) == len(routes), "duplicate route")
    entries, ordered_universe = validate_file_universe(source, suite["file_universe"])
    universe = set(entries)
    oracle_index: source_oracle.SourceOracleIndex | None = None
    literal_index: literal_source_oracle.LiteralSourceOracleIndex | None = None
    require(
        sha(suite["file_universe_digest"], "file_universe_digest")
        == universe_digest(ordered_universe),
        "file universe digest mismatch",
    )
    allowlist: frozenset[tuple[str, int, int]] = frozenset()
    if "leakage_allowlist" in suite:
        allowlist = validate_leakage_allowlist(source, suite["leakage_allowlist"])
    require(isinstance(tasks, list) and bool(tasks), "suite requires tasks")
    # Explicit typo queries use the exhaustive folded-token collision index.
    # Add a submitted typo only when its near-name parser exclusion needs a
    # query-bound key in the same source-oracle index.
    oracle_names = {
        raw.get("intended_name", raw["query"])
        for raw in tasks
        if isinstance(raw, dict)
        and isinstance(raw.get("source_oracle"), dict)
        and raw["source_oracle"].get("contract") != literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT
        and isinstance(raw.get("query"), str)
    }
    oracle_names.update(
        raw["query"]
        for raw in tasks
        if isinstance(raw, dict)
        and isinstance(raw.get("source_oracle"), dict)
        and "near_declaration_exclusions" in raw["source_oracle"]
        and isinstance(raw.get("query"), str)
    )
    literal_names = {
        raw["query"]
        for raw in tasks
        if isinstance(raw, dict)
        and isinstance(raw.get("source_oracle"), dict)
        and raw["source_oracle"].get("contract") == literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT
        and isinstance(raw.get("query"), str)
    }
    declaration_exclusions: dict[tuple[str, str], set[str]] = {}
    for raw in tasks:
        if not isinstance(raw, dict) or not isinstance(raw.get("source_oracle"), dict):
            continue
        annotation = raw["source_oracle"]
        for field in ("declaration_exclusions", "near_declaration_exclusions"):
            if field not in annotation:
                continue
            paths = annotation[field]
            name_contract = source_oracle.NAME_CONTRACTS.get(annotation.get("contract"))
            require(
                name_contract is not None
                and isinstance(raw.get("query"), str)
                and isinstance(paths, list)
                and bool(paths)
                and all(isinstance(path, str) and path in universe for path in paths)
                and paths == sorted(set(paths))
                and (
                    field == "declaration_exclusions"
                    or (isinstance(raw.get("intended_name"), str) and name_contract[1] == "exact")
                ),
                "invalid " + field.replace("_", " "),
            )
            if field == "declaration_exclusions":
                key = (annotation["contract"], raw.get("intended_name", raw["query"]))
            else:
                near_contract = next(
                    contract
                    for contract, owner in source_oracle.NAME_CONTRACTS.items()
                    if owner == (name_contract[0], "osa1_casefold")
                )
                key = (near_contract, raw["query"])
            excluded = set(paths)
            if key in declaration_exclusions:
                require(
                    declaration_exclusions[key] == excluded,
                    "conflicting declaration exclusion query",
                )
            else:
                declaration_exclusions[key] = excluded
    seen_ids = set()
    seen_queries = set()
    eval_count = 0
    families: dict[str, set[str]] = {}
    queries: list[tuple[str, str]] = []
    labels_by_split: dict[str, set[tuple[str, int, int]]] = {"train": set(), "eval": set()}
    scored_files_by_split: dict[str, set[str]] = {"train": set(), "eval": set()}
    judged_files_by_split: dict[str, set[str]] = {"train": set(), "eval": set()}
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
            [
                "category",
                "query_intent",
                "label_review",
                "judgment_policy",
                "answerability_min_grade",
                "file_judgments",
                "declaration_judgments",
                "source_oracle",
                "evaluation_contract",
                "intended_name",
            ],
            "task",
        )
        task_id = string(task["task_id"], "task_id")
        require(task_id not in seen_ids, "duplicate task_id: " + task_id)
        seen_ids.add(task_id)
        require(
            not any(
                field in task
                for field in (
                    "query_intent",
                    "label_review",
                    "judgment_policy",
                    "answerability_min_grade",
                    "file_judgments",
                    "declaration_judgments",
                    "source_oracle",
                    "evaluation_contract",
                    "intended_name",
                )
            )
            or suite.get("diagnostic_policy") == OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
            "task annotations require observed-prefix diagnostic_policy: " + task_id,
        )
        require(task["split"] in ("train", "eval"), "invalid task split: " + task_id)
        if "category" in task:
            string(task["category"], "category for " + task_id)
        if "query_intent" in task:
            require(
                task["query_intent"] in QUERY_INTENTS,
                "invalid query_intent for " + task_id,
            )
        if "evaluation_contract" in task:
            require(
                "query_intent" in task and "judgment_policy" in task,
                f"evaluation_contract requires query intent and judgment policy: {task_id}",
            )
            validate_evaluation_contract(task, task_id)
        if "source_oracle" in task:
            oracle = object_keys_optional(
                task["source_oracle"],
                ["contract", "unit"],
                ["declaration_exclusions", "near_declaration_exclusions"],
                "source_oracle",
            )
            literal_contract = (
                oracle["contract"] == literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT
            )
            required_intent = (
                "exact_content"
                if literal_contract
                else "symbol_components"
                if task.get("evaluation_contract", {}).get("request_mode")
                == query_plan_contract.EXPLICIT_SYMBOL_COMPONENTS
                else "bare_symbol"
            )
            require(
                task.get("query_intent") == required_intent,
                f"source oracle requires {required_intent} intent: {task_id}",
            )
            if literal_contract:
                require(
                    suite["routes"] == ["lexical"] and oracle["unit"] == "distinct_file",
                    f"exact-content source oracle requires Quanta lexical file diagnostic: {task_id}",
                )
            expected_kind = (
                "declaration_judgments" if oracle["unit"] == "symbol" else "file_judgments"
            )
            require(
                oracle["unit"] in ("symbol", "distinct_file")
                and set(task).intersection(("file_judgments", "declaration_judgments"))
                == {expected_kind},
                f"source oracle judgment unit mismatch: {task_id}",
            )
        if "label_review" in task:
            review = object_keys_optional(
                task["label_review"],
                ["assessment"],
                ["reviewer_id", "evidence_sha256"],
                "label_review for " + task_id,
            )
            require(
                review["assessment"] in LABEL_REVIEW_ASSESSMENTS,
                "invalid label_review assessment for " + task_id,
            )
            reviewed = review["assessment"] != "unreviewed"
            require(
                ("reviewer_id" in review) == reviewed and ("evidence_sha256" in review) == reviewed,
                "reviewed label requires reviewer_id and evidence_sha256; unreviewed label forbids them: "
                + task_id,
            )
            if reviewed:
                string(review["reviewer_id"], "label_review.reviewer_id for " + task_id)
                sha(review["evidence_sha256"], "label_review.evidence_sha256 for " + task_id)
        query = string(task["query"], "query")
        oracle_query = query
        if "intended_name" in task:
            intended = string(task["intended_name"], f"intended_name for {task_id}")
            oracle = task.get("source_oracle") or {}
            name_contract = source_oracle.NAME_CONTRACTS.get(oracle.get("contract"))
            require(
                task.get("evaluation_contract", {}).get("request_mode")
                in (query_plan_contract.DEFAULT_FILE_SEARCH, query_plan_contract.EXPLICIT_OSA1_TYPO)
                and task.get("query_intent") == "bare_symbol"
                and name_contract is not None
                and name_contract[1] == "exact"
                and oracle.get("unit") == "distinct_file",
                f"intended_name requires a file-mode exact-name source oracle: {task_id}",
            )
            require(
                query.casefold() != intended.casefold()
                and source_oracle.osa_distance_at_most_one(query.casefold(), intended.casefold()),
                f"intended_name must be one casefold OSA edit from submitted query: {task_id}",
            )
            oracle_query = intended
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
        validate_judgments(source, task, universe, task_id)
        if "source_oracle" in task:
            try:
                if (
                    task["source_oracle"]["contract"]
                    == literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT
                ):
                    if literal_index is None:
                        literal_index = literal_source_oracle.LiteralSourceOracleIndex(
                            {
                                path: (source.file(path)[0], entries[path])
                                for path in sorted(universe)
                            },
                            literal_names,
                        )
                    active_oracle = literal_index
                else:
                    if oracle_index is None:
                        oracle_index = source_oracle.SourceOracleIndex(
                            {
                                path: (source.file(path)[0], entries[path])
                                for path in sorted(universe)
                            },
                            oracle_names,
                            declaration_exclusions,
                            census_cache=declaration_census_cache,
                        )
                    active_oracle = oracle_index
                    if "intended_name" in task:
                        partition = oracle_index.typo_gold_partition(
                            name_contract[0], query, oracle_query
                        )
                        require(
                            not partition["exact_content_collision_paths"]
                            and not partition["query_is_declaration_name"],
                            f"intended_name query collides with a source identifier: {task_id}",
                        )
                oracle_args = (
                    task["source_oracle"]["contract"],
                    oracle_query,
                    task["source_oracle"]["unit"],
                )
                with_names = task["source_oracle"]["unit"] == "symbol" and any(
                    "name_span" in row for row in task.get("declaration_judgments", [])
                )
                expected = (
                    active_oracle.expected_rows(*oracle_args, include_name_spans=True)
                    if with_names
                    else active_oracle.expected_rows(*oracle_args)
                )
            except (
                source_oracle.SourceOracleError,
                literal_source_oracle.LiteralOracleError,
            ) as exc:
                raise EvidenceError(
                    f"source oracle derivation failed for {task_id}: {exc}"
                ) from exc
            kind = (
                "declaration_judgments"
                if task["source_oracle"]["unit"] == "symbol"
                else "file_judgments"
            )
            require(
                sorted(task[kind], key=lambda row: (row["path"], row.get("start_byte", -1)))
                == expected,
                f"source oracle judgments differ from frozen source: {task_id}",
            )
            require(
                task["answerable"] == bool(expected),
                f"source oracle answerability mismatch: {task_id}",
            )
        for kind in ("file_judgments", "declaration_judgments"):
            for judgment in task.get(kind, []):
                if judgment["grade"] > 0:
                    judged_files_by_split[task["split"]].add(judgment["path"])
                    scored_files_by_split[task["split"]].add(judgment["path"])
        labels = task["gold"]
        require(isinstance(labels, list), "gold must be a list: " + task_id)
        require(task["answerable"] == bool(labels), "answerable/gold mismatch: " + task_id)
        if "source_oracle" in task:
            require(
                all(label["path"] in {row["path"] for row in expected} for label in labels),
                f"gold path contradicts source oracle: {task_id}",
            )
            oracle_spans = (
                oracle_index.declaration_name_spans(task["source_oracle"]["contract"], oracle_query)
                if task["source_oracle"]["contract"] in source_oracle.DECLARATION_NAME_CONTRACTS
                else []
            )
            literal_spans = (
                literal_index.spans(query)
                if task["source_oracle"]["contract"]
                == literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT
                else []
            )
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
            if "answerability_min_grade" in task:
                require(
                    label.get("grade", 1) >= answerability_min_grade(task, task_id),
                    "gold grade below answerability_min_grade: " + task_id,
                )
            if "source_oracle" in task:
                if (
                    task["source_oracle"]["contract"]
                    == literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT
                ):
                    matched = any(
                        path == label["path"]
                        and label["start_byte"] <= start
                        and end <= label["end_byte"]
                        for path, start, end in literal_spans
                    )
                elif oracle_spans:
                    matched = any(
                        path == label["path"]
                        and label["start_byte"] <= name_start
                        and name_end <= label["end_byte"]
                        for path, name_start, name_end in oracle_spans
                    )
                else:
                    raw = source.file(label["path"])[0]
                    matched = source_oracle.has_identifier_word_in_span(
                        raw,
                        query.encode("ascii"),
                        label["start_byte"],
                        label["end_byte"],
                    )
                require(matched, f"gold span contradicts source oracle: {task_id}")
            key = (label["path"], label["start_line"], label["end_line"])
            require(key not in seen_labels, "duplicate gold label: " + task_id)
            seen_labels.add(key)
            labels_by_split[task["split"]].add(key)
            scored_files_by_split[task["split"]].add(label["path"])
        if "source_oracle" in task:
            require(
                labels
                == source_oracle_gold(
                    source, active_oracle, task["source_oracle"]["contract"], oracle_query
                ),
                f"source oracle gold differs from canonical first match: {task_id}",
            )
        if task["split"] == "eval":
            eval_count += 1
    require(eval_count > 0, "eval split requires at least one task")
    declared_evaluation_contract(tasks)
    eval_tasks = [task for task in tasks if task["split"] == "eval"]
    for kind in ("file_judgments", "declaration_judgments"):
        if any(kind in task for task in eval_tasks):
            require(
                all(kind in task for task in eval_tasks),
                f"partial {kind} coverage in eval split",
            )
            policies = {task["judgment_policy"] for task in eval_tasks}
            mixed_objective_reviewed = policies == {
                SOURCE_ORACLE_JUDGMENT_POLICY,
                COMPLETE_JUDGMENT_POLICY,
            } and all(
                ("source_oracle" in task)
                == (task["judgment_policy"] == SOURCE_ORACLE_JUDGMENT_POLICY)
                for task in eval_tasks
            )
            require(
                len(policies) == 1 or mixed_objective_reviewed,
                f"mixed judgment_policy for {kind} in eval split",
            )
    for family, splits in sorted(families.items()):
        require(
            len(splits) == 1,
            f"query family spans train and eval: {family}",
        )
    check_query_near_duplicates(queries)
    require(
        not (
            judged_files_by_split["train"] & scored_files_by_split["eval"]
            or judged_files_by_split["eval"] & scored_files_by_split["train"]
        ),
        "independent judgment file leakage across train/eval split",
    )
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
    dev_suite, _dev_pack, dev_source = validate_suite(repo, development)
    holdout_suite, _holdout_pack, holdout_source = validate_suite(repo, holdout)
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

    def scored_files(suite: dict[str, Any]) -> set[str]:
        return {
            row["path"]
            for task in suite["tasks"]
            for row in (
                *task["gold"],
                *task.get("file_judgments", []),
                *task.get("declaration_judgments", []),
            )
            if row.get("grade", 1) > 0
        }

    dev_files = scored_files(dev_suite)
    holdout_files = scored_files(holdout_suite)
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

    def scored_content(suite: dict[str, Any], source: SourceSnapshot) -> set[str]:
        fingerprints = {gold["block_sha256"] for task in suite["tasks"] for gold in task["gold"]}
        for task in suite["tasks"]:
            fingerprints.update(
                row["file_sha256"] for row in task.get("file_judgments", []) if row["grade"] > 0
            )
            for row in task.get("declaration_judgments", []):
                if row["grade"] > 0:
                    raw, _lines, _sha = source.file(row["path"])
                    fingerprints.add(digest(raw[row["start_byte"] : row["end_byte"]]))
        return fingerprints

    dev_blocks = scored_content(dev_suite, dev_source)
    holdout_blocks = scored_content(holdout_suite, holdout_source)
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
        if (
            version == 5
            and entry["system"] == "quanta"
            and entry["execution_profile"]["policy"] == "code_search_exact_content_file"
        ):
            require(
                entry["source_revision_id"] == suite["repository_commit"],
                f"captures.{capture_id}.source_revision_id differs from frozen repository",
            )
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
    if version == 5 and any(
        entry["system"] == "quanta" and entry["execution_profile"]["policy"] == "exact_symbol_name"
        for entry in captures.values()
    ):
        require(
            set(provenance) == {"symbol"},
            "exact_symbol_name profile requires only the symbol route",
        )
    for route, entry in provenance.items():
        item = object_keys(entry, ["capture_id"], f"route_provenance.{route}")
        capture_id = string(item["capture_id"], f"route_provenance.{route}.capture_id")
        require(
            capture_id in captures,
            f"route_provenance.{route} references unknown capture_id: {capture_id}",
        )
    evaluation_contract = declared_evaluation_contract(suite["tasks"])
    if evaluation_contract is not None:
        require(version == RUNNER_SCHEMA_VERSION, "evaluation_contract requires runner v5")
        mode = evaluation_contract["request_mode"]
        expected_policies = query_plan_contract.QUANTA_EVALUATION_POLICIES[mode]
        allowed_routes = (
            {"lexical", "semble-lexical-file"}
            if mode
            in (
                query_plan_contract.DEFAULT_FILE_SEARCH,
                query_plan_contract.NATURAL_LANGUAGE_FILE_SEARCH,
            )
            else {"symbol"}
            if mode == query_plan_contract.DECLARATION_NAVIGATION
            else {"lexical"}
        )
        require(
            bool(provenance) and set(provenance) <= allowed_routes,
            f"{mode} has an unsupported route",
        )
        for route, entry in provenance.items():
            capture = captures[entry["capture_id"]]
            require(
                (
                    route in ("lexical", "symbol")
                    and capture["system"] == "quanta"
                    and capture["execution_profile"]["policy"] in expected_policies
                )
                or (
                    mode
                    in (
                        query_plan_contract.DEFAULT_FILE_SEARCH,
                        query_plan_contract.NATURAL_LANGUAGE_FILE_SEARCH,
                    )
                    and route == "semble-lexical-file"
                    and capture["system"] == "semble"
                    and capture["execution_profile"].get("mode") == "lexical-file"
                ),
                f"{mode} request mode differs from bound product policy: {route}",
            )
    if any(task.get("query_intent") == "exact_content" for task in suite["tasks"]):
        require(
            suite["routes"] == ["lexical"]
            and captures[provenance["lexical"]["capture_id"]]["execution_profile"]["policy"]
            == "code_search_exact_content_file",
            "exact-content suite requires its bound Quanta request policy",
        )
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
    score_evidence_by_route: dict[str, str | None] = {}
    file_depth_by_capture: dict[str, int] = {}
    for raw in results:
        result = (
            object_keys_optional(
                raw,
                result_keys,
                ["rank_unit", "ordering", "score_evidence", "file_collection"],
                "result",
            )
            if version == RUNNER_SCHEMA_VERSION
            else object_keys(raw, result_keys, "result")
        )
        key = (string(result["task_id"], "result.task_id"), string(result["route"], "result.route"))
        require(key in expected and key not in found, f"unexpected/duplicate task route: {key}")
        found.add(key)
        capture = captures[provenance[key[1]]["capture_id"]]
        profile_policy = (
            capture["execution_profile"]["policy"]
            if version == RUNNER_SCHEMA_VERSION and capture["system"] == "quanta"
            else None
        )
        semble_file = (
            version == RUNNER_SCHEMA_VERSION
            and capture["system"] == "semble"
            and capture["execution_profile"]["mode"] == "lexical-file"
        )
        rank_unit = result.get("rank_unit")
        if evaluation_contract is not None:
            require(
                rank_unit == evaluation_contract["result_unit"],
                f"evaluation_contract result_unit mismatch for {key}",
            )
        if rank_unit is not None:
            require(
                rank_unit in ("distinct_file", "symbol"),
                f"unknown result rank_unit for {key}",
            )
        ordering = result.get("ordering")
        score_evidence = result.get("score_evidence")
        if profile_policy in query_plan_contract.FILE_PAIR_POLICIES:
            require(
                span_protocol == 1,
                f"code_search_file requires source-bound file span evidence: {key}",
            )
            require(
                score_evidence == "native_sdk_score_v1",
                f"code_search_file requires native SDK score evidence: {key}",
            )
        if score_evidence is not None:
            require(
                (
                    profile_policy in query_plan_contract.SCORED_QUANTA_FILE_POLICIES
                    and score_evidence == "native_sdk_score_v1"
                )
                or (semble_file and score_evidence == "semble_bm25_score_v1"),
                f"score evidence requires a scored Quanta file policy with native SDK scores or Semble lexical-file BM25 scores: {key}",
            )
        if profile_policy in query_plan_contract.SCORED_QUANTA_FILE_POLICIES or semble_file:
            if key[1] in score_evidence_by_route:
                require(
                    score_evidence_by_route[key[1]] == score_evidence,
                    f"{profile_policy} route mixes score evidence states: {key[1]}"
                    if profile_policy in query_plan_contract.SCORED_QUANTA_FILE_POLICIES
                    else f"Semble lexical-file route mixes score evidence states: {key[1]}",
                )
            else:
                score_evidence_by_route[key[1]] = score_evidence
        if profile_policy in query_plan_contract.FILE_PROJECTION_ORDERING:
            require(
                key[1] == "lexical" and rank_unit == "distinct_file",
                f"{profile_policy} profile requires lexical distinct_file result: {key}",
            )
            # rank_unit names the result unit only; ordering is derived from the
            # request policy and cannot be relabeled. Historical literal_file
            # records predate the field and keep their derived path order.
            expected_ordering = query_plan_contract.FILE_PROJECTION_ORDERING[profile_policy]
            require(
                ordering == expected_ordering
                or (ordering is None and profile_policy == "literal_file"),
                f"{profile_policy} result ordering must be {expected_ordering}: {key}",
            )
        elif profile_policy == "exact_symbol_name":
            require(
                key[1] == "symbol" and rank_unit in (None, "symbol"),
                f"exact_symbol_name profile requires symbol rank_unit when explicit: {key}",
            )
            if rank_unit == "symbol":
                require(
                    span_protocol == 1,
                    f"symbol rank_unit requires published-unit span protocol: {key}",
                )
        elif semble_file:
            require(
                rank_unit == "distinct_file"
                and ordering == "score_desc_native_tiebreak"
                and score_evidence == "semble_bm25_score_v1",
                f"Semble lexical-file requires a scored distinct-file result: {key}",
            )
        elif capture["system"] == "quanta":
            require(rank_unit is None, f"Quanta chunk profile has incompatible rank_unit: {key}")
        else:
            require(
                rank_unit is None,
                f"Semble capture has no verified distinct_file rank authority: {key}",
            )
        if profile_policy not in query_plan_contract.FILE_PROJECTION_ORDERING and not semble_file:
            require(ordering is None, f"ordering applies only to file projections: {key}")
        if not semble_file:
            require(
                "file_collection" not in result,
                f"file collection requires Semble lexical-file: {key}",
            )
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
                try:
                    expected_identity = query_plan_contract.derive_query_identity_v4(
                        policy, pack_task["query"], nl_config
                    )
                except query_plan_contract.QueryPlanError as exc:
                    raise EvidenceError(
                        f"query cannot be planned under {policy}: {key}: {exc}"
                    ) from exc
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
                    try:
                        expected_identity = query_plan_contract.derive_query_identity(
                            profile["policy"], pack_task["query"], profile["config"] or None
                        )
                    except query_plan_contract.QueryPlanError as exc:
                        # A query the policy cannot plan can never have produced
                        # this record: a typed refusal, not a traceback.
                        raise EvidenceError(
                            f"query cannot be planned under {profile['policy']}: {key}: {exc}"
                        ) from exc
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
        require(
            len(candidates) <= suite["comparison_contract"]["top_k"],
            f"candidates exceed declared top_k: {key}",
        )
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
        if semble_file:
            collection = result.get("file_collection")
            if status in ("success", "abstained"):
                collection = object_keys(
                    collection,
                    ["indexed_chunks", "matched_chunks", "matching_files"],
                    f"file_collection for {key}",
                )
                indexed = collection["indexed_chunks"]
                matched = collection["matched_chunks"]
                matching_files = collection["matching_files"]
                require(
                    type(indexed) is int
                    and indexed > 0
                    and type(matched) is int
                    and 0 <= matched <= indexed
                    and type(matching_files) is int
                    and 0 <= matching_files <= matched,
                    f"Semble file collection counts are invalid: {key}",
                )
                require(
                    len(candidates) == min(matching_files, suite["comparison_contract"]["top_k"])
                    and (status == "abstained") == (matched == 0),
                    f"Semble file collection does not explain returned files: {key}",
                )
                capture_id = provenance[key[1]]["capture_id"]
                if capture_id in file_depth_by_capture:
                    require(
                        file_depth_by_capture[capture_id] == indexed,
                        f"Semble indexed chunk depth changes within a capture: {key}",
                    )
                else:
                    file_depth_by_capture[capture_id] = indexed
            else:
                require(collection is None, f"failed Semble file collection has counts: {key}")
        if (
            profile_policy in query_plan_contract.FILE_PROJECTION_ORDERING
            and query_plan_contract.FILE_PROJECTION_ORDERING[profile_policy]
            == query_plan_contract.ORDERING_PATH_ORDER
        ):
            # A match-only restriction ranks every file equally; the engine
            # breaks the tie on repository/path, so any other order is forged.
            # Duplicates are refused below with their own, more specific error.
            paths = [item.get("path") for item in candidates if isinstance(item, dict)]
            require(
                len(set(paths)) != len(paths)
                or paths == sorted(paths, key=lambda path: str(path).encode("utf-8")),
                f"path-ordered file projection is not in path order: {key}",
            )
        # The exact-symbol contract can retain distinct indexed units sharing
        # one scored line. Native and content profiles keep context collapse.
        seen_spans: dict[tuple[str, int, int], set[tuple[int, int]] | None] = {}
        seen_unit_ids: set[str] = set()
        seen_files: set[str] = set()
        previous_scored_file: tuple[float, str] | None = None
        for index, candidate in enumerate(candidates, start=1):
            block(
                source,
                candidate,
                f"candidate for {key}",
                candidate=True,
                universe=universe,
                allow_span_accounting=version == 5 and capture["system"] == "quanta",
                allow_score=score_evidence in ("native_sdk_score_v1", "semble_bm25_score_v1"),
            )
            if (
                profile_policy in ("code_search_file", "code_search_exact_content_file")
                and "source_repo_id" in capture
            ):
                accounting = candidate["span_accounting"]
                require(
                    accounting["source_repo_id"] == capture["source_repo_id"]
                    and accounting["source_revision_id"] == capture["source_revision_id"],
                    f"file candidate source pin differs from capture: {key}",
                )
            if score_evidence in ("native_sdk_score_v1", "semble_bm25_score_v1"):
                require("score" in candidate, f"missing native SDK score for {key}")
                score = float(candidate["score"])
                path = candidate["path"]
                if previous_scored_file is not None:
                    previous_score, previous_path = previous_scored_file
                    require(
                        score < previous_score
                        or (
                            score == previous_score
                            and (semble_file or path.encode() >= previous_path.encode())
                        ),
                        f"{profile_policy} native SDK score/path order is invalid: {key}"
                        if profile_policy in query_plan_contract.SCORED_QUANTA_FILE_POLICIES
                        else f"Semble lexical-file native score order is invalid: {key}",
                    )
                previous_scored_file = (score, path)
            if "span_accounting" in candidate:
                require(span_protocol == 1, f"span evidence lacks record protocol: {key}")
                accounting = candidate["span_accounting"]
                require(
                    (profile_policy in query_plan_contract.CODE_SEARCH_FILE_POLICIES)
                    == (accounting["unit_kind"] == "file"),
                    f"code_search_file requires file identity and other profiles cannot claim it: {key}",
                )
                if rank_unit == "symbol":
                    require(
                        accounting["unit_kind"] == "symbol",
                        f"symbol rank_unit requires published symbol unit: {key}",
                    )
                expected_producer = (
                    "source-bound-symbols-v2"
                    if accounting["unit_kind"] == "symbol"
                    else "code-search-file-v1"
                    if accounting["unit_kind"] == "file"
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
            if rank_unit == "distinct_file":
                require(
                    candidate["path"] not in seen_files,
                    f"duplicate file in distinct_file result: {key}",
                )
                seen_files.add(candidate["path"])
            span = (
                candidate["path"],
                candidate["start_byte"],
                candidate["end_byte"],
            )
            accounting = candidate.get("span_accounting")
            symbol_span = (
                (accounting["indexed_start_byte"], accounting["indexed_end_byte"])
                if accounting is not None
                and accounting["unit_kind"] == "symbol"
                and profile_policy == "exact_symbol_name"
                and key[1] == "symbol"
                and span_protocol == 1
                else None
            )
            if span in seen_spans:
                require(
                    symbol_span is not None and seen_spans[span] is not None,
                    f"duplicate candidate byte span: {key}",
                )
                require(
                    symbol_span not in seen_spans[span],
                    f"duplicate published symbol indexed span: {key}",
                )
                seen_spans[span].add(symbol_span)
            else:
                seen_spans[span] = {symbol_span} if symbol_span is not None else None
    require(found == expected, f"missing task route evidence: {sorted(expected - found)}")
    return run


def load_evidence(
    repo: Path, suite_path: Path, runner_path: Path
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    suite, pack, source = validate_suite(repo, read_json(suite_path))
    run = validate_evidence_against_suite(repo, suite, pack, source, read_json(runner_path))
    return suite, pack, run


def validate_evidence_against_suite(
    repo: Path,
    suite: dict[str, Any],
    pack: dict[str, Any],
    source: SourceSnapshot,
    payload: Any,
) -> dict[str, Any]:
    """Validate a record using a suite already derived from the frozen source.

    A caller may reuse this context only within one validation pass. A fresh
    verdict must re-derive the suite from its own frozen artifact bytes.
    """
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
    return run


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


def observed_prefix_diagnostics(
    candidates: list[dict[str, Any]],
    labels: list[dict[str, Any]],
    *,
    status: str,
    declared_top_k: int,
    indexed_span_authority: bool,
) -> dict[str, Any]:
    """Describe only ranks present in the recorded top-10 prefix.

    A null rank means no matching candidate was observed here. It never
    asserts the candidate's rank beyond this prefix or in a larger search.
    """
    top = candidates[:MRR_K] if status in SCORED_STATUSES else []
    gold_files = {label["path"] for label in labels}
    first_file = next((item["rank"] for item in top if item["path"] in gold_files), None)
    first_context = next(
        (item["rank"] for item in top if any(covers(item, label) for label in labels)), None
    )
    first_indexed = (
        next(
            (item["rank"] for item in top if any(indexed_covers(item, label) for label in labels)),
            None,
        )
        if indexed_span_authority
        else None
    )
    unique_files = len({item["path"] for item in top})
    return {
        "scope": "recorded_top_10_prefix",
        "declared_top_k": declared_top_k,
        "observed_depth": len(top),
        "result_status": status,
        "unique_files": unique_files,
        "duplicate_file_candidates": len(top) - unique_files,
        "first_gold_file_rank": first_file,
        "first_gold_returned_context_span_rank": first_context,
        "first_gold_indexed_span_rank": first_indexed,
        "indexed_span_authority": (
            "published_unit_v1" if indexed_span_authority else "unavailable"
        ),
    }


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
        if (
            run["captures"][capture_id].get("execution_profile", {}).get("policy")
            in query_plan_contract.FILE_PAIR_POLICIES
        ):
            routes[route] = {"status": "not_applicable", "reason": "file_unit_is_not_context_span"}
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


def file_ndcg_at_k(
    candidates: list[dict[str, Any]], judgments: list[dict[str, Any]], k: int
) -> float:
    """Rank files against independent source-bound grades, without span-density gain."""
    grades = {row["path"]: row["grade"] for row in judgments}
    ideal = sorted((grade for grade in grades.values() if grade > 0), reverse=True)[:k]
    idcg = sum((2**grade - 1) / math.log2(rank + 1) for rank, grade in enumerate(ideal, 1))
    if idcg == 0:
        return 0.0
    seen: set[str] = set()
    dcg = 0.0
    for rank, item in enumerate(candidates[:k], 1):
        path = item["path"]
        if path in seen:
            continue
        seen.add(path)
        dcg += (2 ** grades.get(path, 0) - 1) / math.log2(rank + 1)
    score = dcg / idcg
    require(-1e-12 <= score <= 1 + 1e-12, "file NDCG exceeds its ideal bound")
    return min(1.0, max(0.0, score))


def file_hit_at_k_judged(
    candidates: list[dict[str, Any]], judgments: list[dict[str, Any]], k: int
) -> float:
    """1.0 when any positively graded file is among the first k returned files."""
    positive = {row["path"] for row in judgments if row["grade"] > 0}
    return float(any(item["path"] in positive for item in candidates[:k]))


def file_mrr_at_k_judged(
    candidates: list[dict[str, Any]], judgments: list[dict[str, Any]], k: int
) -> float:
    """Reciprocal rank of the first positively judged distinct file."""
    positive = {row["path"] for row in judgments if row["grade"] > 0}
    for rank, item in enumerate(candidates[:k], 1):
        if item["path"] in positive:
            return 1.0 / rank
    return 0.0


def file_recall_at_k_judged(
    candidates: list[dict[str, Any]], judgments: list[dict[str, Any]], k: int
) -> float:
    """Share of positively graded files among the first k returned files."""
    positive = {row["path"] for row in judgments if row["grade"] > 0}
    if not positive:
        return 0.0
    return len(positive & {item["path"] for item in candidates[:k]}) / len(positive)


#: How a route's rank metrics may be read, from its bound ordering contract.
RANK_METRIC_INTERPRETATION = {
    "score_desc_path_tiebreak": "scored_ranking",
    "score_desc_native_tiebreak": "scored_ranking_native_ties",
    "path_order_constant_score": "observed_path_order_prefix",
}


def _route_ordering(run: dict[str, Any], route: str, results: dict, task_ids: list[str]) -> str:
    """The ordering contract of one file-projection route, or the chunk rank."""
    capture = run["captures"][run["route_provenance"][route]["capture_id"]]
    policy = capture.get("execution_profile", {}).get("policy")
    derived = query_plan_contract.FILE_PROJECTION_ORDERING.get(policy)
    if (
        capture.get("system") == "semble"
        and capture.get("execution_profile", {}).get("mode") == "lexical-file"
    ):
        derived = "score_desc_native_tiebreak"
    if derived is None:
        return "not_a_file_projection"
    for task_id in task_ids:
        recorded = results[(task_id, route)].get("ordering")
        require(
            recorded in (None, derived),
            f"route {route} ordering differs from its policy contract",
        )
    return derived


def _route_score_evidence(results: dict, route: str, task_ids: list[str]) -> str:
    """Keep legacy contract-only order distinct from a native score proof."""
    if not task_ids:
        return "not_applicable"
    observed = {results[(task_id, route)].get("score_evidence") for task_id in task_ids}
    require(len(observed) == 1, f"route {route} mixes score evidence states")
    return next(iter(observed)) or "not_recorded"


def _declaration_match(candidate: dict[str, Any], judgment: dict[str, Any]) -> bool:
    span = candidate.get("span_accounting")
    return (
        isinstance(span, dict)
        and span.get("unit_kind") == "symbol"
        and isinstance(span.get("unit_id"), str)
        and bool(span["unit_id"].strip())
        and candidate["path"] == judgment["path"]
        and span["indexed_start_byte"] == judgment["start_byte"]
        and span["indexed_end_byte"] == judgment["end_byte"]
    )


def declaration_recall_at_k(
    candidates: list[dict[str, Any]], judgments: list[dict[str, Any]], k: int
) -> float:
    positive = [row for row in judgments if row["grade"] > 0]
    if not positive:
        return 0.0
    return sum(
        any(_declaration_match(item, label) for item in candidates[:k]) for label in positive
    ) / len(positive)


def declaration_mrr_at_k(
    candidates: list[dict[str, Any]], judgments: list[dict[str, Any]], k: int
) -> float:
    positive = [row for row in judgments if row["grade"] > 0]
    for rank, item in enumerate(candidates[:k], 1):
        if any(_declaration_match(item, label) for label in positive):
            return 1.0 / rank
    return 0.0


def _declaration_name_match(candidate: dict[str, Any], judgment: dict[str, Any]) -> bool:
    span = candidate.get("span_accounting", {})
    return (
        _declaration_match(candidate, judgment)
        and isinstance(judgment.get("name_span"), dict)
        and span.get("name_span") == judgment["name_span"]
    )


def declaration_name_recall_at_k(
    candidates: list[dict[str, Any]], judgments: list[dict[str, Any]], k: int
) -> float:
    positive = [row for row in judgments if row["grade"] > 0]
    if not positive:
        return 0.0
    return sum(
        any(_declaration_name_match(item, label) for item in candidates[:k]) for label in positive
    ) / len(positive)


def declaration_name_mrr_at_k(
    candidates: list[dict[str, Any]], judgments: list[dict[str, Any]], k: int
) -> float:
    positive = [row for row in judgments if row["grade"] > 0]
    for rank, item in enumerate(candidates[:k], 1):
        if any(_declaration_name_match(item, label) for label in positive):
            return 1.0 / rank
    return 0.0


def judgment_diagnostics(
    suite: dict[str, Any],
    run: dict[str, Any],
    results: dict[tuple[str, str], dict[str, Any]],
    tasks: dict[str, dict[str, Any]],
    baseline: str,
    candidate: str | None,
) -> dict[str, Any] | None:
    """Opt-in operational and common-cohort quality views for source-bound judgments."""
    kinds = [
        kind
        for kind in ("file_judgments", "declaration_judgments")
        if any(kind in task for task in tasks.values())
    ]
    if not kinds:
        return None
    if "declaration_judgments" in kinds:
        kinds.append("declaration_name_recovery")
    require(
        suite["comparison_contract"]["top_k"] >= NDCG_K,
        "independent judgment diagnostics require top_k >= 10",
    )
    answerable_ids = sorted(task_id for task_id, task in tasks.items() if task["answerable"])
    policies = {task["judgment_policy"] for task in tasks.values() if "judgment_policy" in task}
    if len(policies) == 1:
        policy = next(iter(policies))
    else:
        policy = "mixed" if policies else UNJUDGED_POLICY
    output: dict[str, Any] = {
        "unjudged_policy": policy,
    }
    if any("answerability_min_grade" in task for task in tasks.values()):
        output["answerability_min_grade_by_task"] = {
            task_id: answerability_min_grade(task, task_id) for task_id, task in tasks.items()
        }
    evaluation_contract = declared_evaluation_contract(list(tasks.values()))
    for kind in kinds:
        judgment_kind = "declaration_judgments" if kind == "declaration_name_recovery" else kind
        if kind == "file_judgments":
            expected_unit = "distinct_file"
            # Hit/recall read the observed top 10 under any ordering; NDCG is a
            # ranking-quality number only on a scored ordering.
            metrics = {
                "ndcg_at_10": file_ndcg_at_k,
                "hit_at_10": file_hit_at_k_judged,
                "recall_at_10": file_recall_at_k_judged,
            }
            if evaluation_contract is not None:
                metrics["mrr_at_10"] = file_mrr_at_k_judged
        else:
            expected_unit = "symbol"
            metrics = {
                "recall_at_10": declaration_recall_at_k,
                "mrr_at_10": declaration_mrr_at_k,
            }
            if kind == "declaration_name_recovery":
                metrics = {
                    "recall_at_10": declaration_name_recall_at_k,
                    "mrr_at_10": declaration_name_mrr_at_k,
                }
        by_route: dict[str, Any] = {}
        eligible_scores: dict[str, dict[str, dict[str, float]]] = {}
        per_query: list[dict[str, Any]] = []
        for route in suite["routes"]:
            score_by_task: dict[str, dict[str, float]] = {}
            excluded: list[dict[str, str]] = []
            status_counts: dict[str, int] = {}
            operational = {metric: 0.0 for metric in metrics}
            conditional = {metric: 0.0 for metric in metrics}
            for task_id in answerable_ids:
                result = results[(task_id, route)]
                status = _result_status(result)
                status_counts[status] = status_counts.get(status, 0) + 1
                ranked = _ordered_candidates(result)
                rank_unit = result.get("rank_unit")
                capture_id = run["route_provenance"][route]["capture_id"]
                capture = run["captures"][capture_id]
                system = capture["system"]
                complete_semble_file = (
                    system == "semble"
                    and rank_unit == "distinct_file"
                    and result.get("file_collection") is not None
                )
                reason = None
                if judgment_kind not in tasks[task_id]:
                    reason = "missing_independent_judgments"
                elif kind == "declaration_name_recovery" and not all(
                    "name_span" in row for row in tasks[task_id][judgment_kind]
                ):
                    reason = "missing_independent_name_gold"
                elif rank_unit != expected_unit:
                    reason = "rank_unit_mismatch"
                elif status not in SCORED_STATUSES and not (
                    status == "abstained" and (system == "quanta" or complete_semble_file)
                ):
                    reason = "execution_status_" + status
                elif judgment_kind == "declaration_judgments" and (
                    run.get("span_accounting_version") != 1
                    or system != "quanta"
                    or any(
                        item.get("span_accounting", {}).get("unit_kind") != "symbol"
                        for item in ranked
                    )
                ):
                    reason = "missing_published_symbol_authority"
                elif kind == "declaration_name_recovery" and any(
                    not isinstance(item.get("span_accounting", {}).get("name_span"), dict)
                    for item in ranked
                ):
                    reason = "missing_published_name_authority"
                elif len(ranked) < NDCG_K and not (
                    status in ("success", "abstained")
                    and (system == "quanta" or complete_semble_file)
                ):
                    reason = "insufficient_depth_without_exhaustion"
                elif tasks[task_id].get("judgment_policy") == COMPLETE_JUDGMENT_POLICY:
                    judgments = tasks[task_id][judgment_kind]
                    if kind == "file_judgments":
                        judged_files = {item["path"] for item in judgments}
                        if any(item["path"] not in judged_files for item in ranked[:NDCG_K]):
                            reason = "unjudged_ranked_file"
                    else:
                        judged_declarations = {
                            (item["path"], item["start_byte"], item["end_byte"])
                            for item in judgments
                        }
                        if any(
                            (
                                item["path"],
                                item["span_accounting"]["indexed_start_byte"],
                                item["span_accounting"]["indexed_end_byte"],
                            )
                            not in judged_declarations
                            for item in ranked[:NDCG_K]
                        ):
                            reason = "unjudged_ranked_declaration"
                if reason is not None:
                    excluded.append({"task_id": task_id, "reason": reason})
                    per_query.append(
                        {"task_id": task_id, "route": route, "eligible": False, "reason": reason}
                    )
                    continue
                values = {
                    metric: scorer(ranked, tasks[task_id][judgment_kind], NDCG_K)
                    for metric, scorer in metrics.items()
                }
                score_by_task[task_id] = values
                for metric, value in values.items():
                    operational[metric] += value
                    conditional[metric] += value
                per_query.append(
                    {"task_id": task_id, "route": route, "eligible": True, "scores": values}
                )
            eligible_scores[route] = score_by_task
            ordering = (
                _route_ordering(run, route, results, answerable_ids)
                if kind == "file_judgments"
                else "symbol_rank"
            )
            missing_ranked_judgments = any(
                item["reason"] in ("unjudged_ranked_file", "unjudged_ranked_declaration")
                for item in excluded
            )
            missing_name_authority = kind == "declaration_name_recovery" and any(
                item["reason"]
                in (
                    "missing_independent_judgments",
                    "missing_independent_name_gold",
                    "missing_published_symbol_authority",
                    "missing_published_name_authority",
                    "rank_unit_mismatch",
                )
                for item in excluded
            )
            by_route[route] = {
                "rank_unit": expected_unit,
                "ordering": ordering,
                "score_evidence": (
                    _route_score_evidence(results, route, answerable_ids)
                    if kind == "file_judgments"
                    and ordering in ("score_desc_path_tiebreak", "score_desc_native_tiebreak")
                    else "not_applicable"
                ),
                "rank_metric_interpretation": RANK_METRIC_INTERPRETATION.get(
                    ordering, "not_a_file_projection" if kind == "file_judgments" else "symbol_rank"
                ),
                "selected_answerable_tasks": len(answerable_ids),
                "eligible_task_ids": sorted(score_by_task),
                "eligible_count": len(score_by_task),
                "coverage": len(score_by_task) / len(answerable_ids) if answerable_ids else 0.0,
                "excluded": excluded,
                "status_counts": status_counts,
                "operational_mean": {
                    metric: (
                        value / len(answerable_ids)
                        if answerable_ids
                        and not missing_ranked_judgments
                        and not missing_name_authority
                        else NOT_APPLICABLE
                    )
                    for metric, value in operational.items()
                },
                "conditional_mean": {
                    metric: value / len(score_by_task) if score_by_task else NOT_APPLICABLE
                    for metric, value in conditional.items()
                },
            }
            if missing_ranked_judgments:
                by_route[route]["operational_unavailable_reason"] = "incomplete_ranked_judgments"
            elif missing_name_authority:
                by_route[route]["operational_unavailable_reason"] = "incomplete_name_authority"
        if candidate is None:
            comparison: dict[str, Any] | str = NOT_APPLICABLE
        else:
            common = sorted(set(eligible_scores[baseline]) & set(eligible_scores[candidate]))
            comparison = {
                "baseline": baseline,
                "candidate": candidate,
                "qualification": "diagnostic_only_no_coverage_floor",
                "eligible_task_ids": common,
                "sample_count": len(common),
                "coverage": len(common) / len(answerable_ids) if answerable_ids else 0.0,
                "delta": {},
                "ci_95": {},
            }
            for metric in metrics:
                deltas = [
                    eligible_scores[candidate][task_id][metric]
                    - eligible_scores[baseline][task_id][metric]
                    for task_id in common
                ]
                comparison["delta"][metric] = (
                    sum(deltas) / len(deltas) if deltas else NOT_APPLICABLE
                )
                comparison["ci_95"][metric] = mean_ci(
                    deltas,
                    [
                        (task_id, str(tasks[task_id].get("category", "uncategorized")))
                        for task_id in common
                    ],
                )
        output[kind] = {
            "routes": by_route,
            "comparison": comparison,
            "per_query": sorted(per_query, key=lambda row: (row["task_id"], row["route"])),
        }
    return output


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
    # The 1,196-query diagnostic exceeds 64 KiB and is re-scored by the
    # independent in-process verdict. Retain only bounded canonical keys;
    # source/evidence validation is never cached.
    bounds = _bootstrap_bounds if len(seed_bytes) <= 262_144 else _bootstrap_bounds.__wrapped__
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


@lru_cache(maxsize=16)
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


def repository_cluster_ci(
    rows: list[tuple[str, str, str, float]],
    release_digest: str,
    repositories: dict[str, str],
    repository_strata: dict[str, str],
) -> dict[str, Any]:
    """Bootstrap repository means within frozen sampling strata.

    The caller must separately prove paired, judged task coverage and bind this
    release to its captures. This interval alone never qualifies a decision.
    """
    require(
        isinstance(release_digest, str) and bool(HEX64_RE.fullmatch(release_digest)),
        "cluster release digest is invalid",
    )
    require(
        isinstance(repositories, dict) and bool(repositories),
        "cluster repository inventory is empty",
    )
    for name, commit in repositories.items():
        require(
            isinstance(name, str)
            and bool(name)
            and isinstance(commit, str)
            and bool(COMMIT_RE.fullmatch(commit)),
            "cluster repository is invalid",
        )
    require(
        isinstance(repository_strata, dict) and set(repository_strata) == set(repositories),
        "cluster repository strata inventory differs",
    )
    require(
        all(isinstance(value, str) and value for value in repository_strata.values()),
        "cluster repository stratum is invalid",
    )
    families: dict[str, dict[str, list[float]]] = {name: {} for name in repositories}
    seen_tasks: set[tuple[str, str]] = set()
    family_owner: dict[str, str] = {}
    require(isinstance(rows, list), "cluster rows must be a list")
    for row in rows:
        require(isinstance(row, tuple) and len(row) == 4, "cluster row is malformed")
        repository, task_id, family_id, delta = row
        require(isinstance(repository, str), "cluster repository is invalid")
        require(repository in repositories, "cluster row has an unknown repository")
        require(
            isinstance(task_id, str)
            and bool(task_id)
            and isinstance(family_id, str)
            and bool(family_id),
            "cluster task or family is empty",
        )
        require((repository, task_id) not in seen_tasks, "cluster task is duplicated")
        require(type(delta) in (int, float) and math.isfinite(delta), "cluster delta is not finite")
        require(-1.0 <= delta <= 1.0, "cluster bounded metric delta is out of range")
        require(
            family_owner.setdefault(family_id, repository) == repository,
            "cluster family crosses repositories",
        )
        seen_tasks.add((repository, task_id))
        families[repository].setdefault(family_id, []).append(delta)
    require(all(families.values()), "cluster repository lacks paired rows")
    means = [
        math.fsum(
            math.fsum(sorted(families[name][family_id])) / len(families[name][family_id])
            for family_id in sorted(families[name])
        )
        / len(families[name])
        for name in sorted(repositories)
    ]
    groups: dict[str, list[float]] = {}
    for name, mean in zip(sorted(repositories), means, strict=True):
        groups.setdefault(repository_strata[name], []).append(mean)
    reason = (
        "insufficient_independent_repositories"
        if len(means) < 12
        else "insufficient_repositories_in_stratum"
        if any(len(group) < 2 for group in groups.values())
        else None
    )
    summary: dict[str, Any] = {
        "method": "paired_stratified_repository_cluster_bootstrap_percentile_v1",
        "status": NOT_APPLICABLE if reason else "available",
        "repository_count": len(means),
        "family_count": sum(map(len, families.values())),
        "sample_count": len(rows),
        "aggregation": "equal_family_within_repository_equal_repository",
        "strata": {name: len(group) for name, group in sorted(groups.items())},
    }
    if reason:
        return {**summary, "reason": reason}
    seed = canonical(
        {
            "release_digest": release_digest,
            "repositories": [
                {
                    "name": name,
                    "commit": repositories[name],
                    "stratum": repository_strata[name],
                    "mean": mean,
                }
                for name, mean in zip(sorted(repositories), means, strict=True)
            ],
        }
    )
    rng = random.Random(int(digest(seed)[:16], 16))
    sampled = sorted(
        math.fsum(
            groups[stratum][rng.randrange(len(groups[stratum]))]
            for stratum in sorted(groups)
            for _ in groups[stratum]
        )
        / len(means)
        for _ in range(10_000)
    )

    def quantile(p: float) -> float:
        position = p * (len(sampled) - 1)
        lower, upper = math.floor(position), math.ceil(position)
        weight = position - lower
        return sampled[lower] * (1.0 - weight) + sampled[upper] * weight

    return {
        **summary,
        "resamples": 10_000,
        "seed_sha256": digest(seed),
        "mean": math.fsum(means) / len(means),
        "lower_95": quantile(0.025),
        "upper_95": quantile(0.975),
    }


def paired_query_family_rows(
    suite: dict[str, Any],
    report: dict[str, Any],
    baseline: str,
    candidate: str,
    *,
    require_graded: bool = False,
) -> list[tuple[str, str, str, float]]:
    """Re-derive paired deltas from a suite-bound report.

    The caller must first replay the capture and validate the suite against its
    source. A matching suite digest alone does not establish that authority.
    """
    require(isinstance(suite, dict) and isinstance(report, dict), "cluster inputs are malformed")
    rank_metrics = report.get("rank_metrics")
    comparison = rank_metrics.get("comparison") if isinstance(rank_metrics, dict) else None
    require(isinstance(comparison, dict), "cluster comparison is missing")
    primary = comparison.get("primary_metric")
    require(
        isinstance(primary, str) and primary in {"ndcg_at_10", "recall_at_10", "file_ndcg_at_10"},
        "unsupported cluster primary metric",
    )
    require(
        isinstance(baseline, str)
        and bool(baseline)
        and isinstance(candidate, str)
        and bool(candidate)
        and baseline != candidate
        and comparison.get("baseline") == baseline
        and comparison.get("candidate") == candidate,
        "cluster comparison routes differ",
    )
    if require_graded:
        require(report.get("graded") is True, "cluster report is not graded")
    require(
        report.get("repository_commit") == suite.get("repository_commit")
        and report.get("suite_id") == suite.get("suite_id")
        and report.get("suite_commitment_sha256") == digest(canonical(suite)),
        "cluster report suite binding differs",
    )
    tasks = suite.get("tasks")
    per_query = report.get("per_query")
    require(isinstance(tasks, list) and isinstance(per_query, list), "cluster tasks are malformed")
    require(
        all(
            isinstance(task, dict)
            and isinstance(task.get("task_id"), str)
            and bool(task["task_id"])
            and isinstance(task.get("split"), str)
            and isinstance(task.get("gold"), list)
            and isinstance(task.get("query_family_id"), str)
            and bool(task["query_family_id"])
            for task in tasks
        ),
        "cluster suite tasks are malformed",
    )
    eval_tasks = {task["task_id"]: task for task in tasks if task["split"] == "eval"}
    require(
        len(eval_tasks) == sum(task["split"] == "eval" for task in tasks),
        "cluster eval task identities are duplicate",
    )
    by_key = {}
    for row in per_query:
        require(
            isinstance(row, dict)
            and isinstance(row.get("task_id"), str)
            and isinstance(row.get("route"), str),
            "cluster query row is malformed",
        )
        key = (row.get("task_id"), row.get("route"))
        require(key not in by_key, "cluster query rows are duplicate")
        by_key[key] = row
    field = {
        "ndcg_at_10": "ndcg_at_10",
        "recall_at_10": "chunk_recall_at_10",
        "file_ndcg_at_10": "file_ndcg_at_10",
    }[primary]
    rows = []
    for task_id, task in sorted(eval_tasks.items()):
        require(
            (task_id, baseline) in by_key and (task_id, candidate) in by_key,
            "cluster report is missing a paired task row",
        )
        if not task["gold"]:
            continue
        before = by_key[(task_id, baseline)].get(field)
        after = by_key[(task_id, candidate)].get(field)
        require(
            is_finite_json_number(before)
            and is_finite_json_number(after)
            and 0 <= before <= 1
            and 0 <= after <= 1,
            "cluster primary metric is not a bounded finite number",
        )
        rows.append(
            (
                task_id,
                task["query_family_id"],
                str(task.get("category", "uncategorized")),
                float(after - before),
            )
        )
    delta = comparison.get("primary_delta")
    require(
        bool(rows)
        and type(comparison.get("sample_count")) is int
        and comparison["sample_count"] == len(rows)
        and is_finite_json_number(delta)
        and math.isclose(math.fsum(row[3] for row in rows) / len(rows), delta, abs_tol=1e-12),
        "cluster comparison differs from paired query rows",
    )
    return rows


def qualified_query_family_ci(
    suite: dict[str, Any], report: dict[str, Any], baseline: str, candidate: str
) -> dict[str, Any]:
    """Derive independent-family uncertainty from the re-scored report rows."""
    rows = paired_query_family_rows(suite, report, baseline, candidate)
    return query_family_cluster_ci(rows, suite["repository_commit"])


def repository_cluster_ci_from_reports(
    suites: dict[str, dict[str, Any]],
    reports: dict[str, dict[str, Any]],
    release_digest: str,
    repositories: dict[str, str],
    repository_strata: dict[str, str],
    baseline: str,
    candidate: str,
) -> dict[str, Any]:
    """Compute repository uncertainty after each native report was replayed.

    This input boundary checks paired rows and repository coverage; it does not
    replace capture, reviewed-label, indexed-universe or custody validation.
    """
    require(
        isinstance(suites, dict)
        and isinstance(reports, dict)
        and set(suites) == set(repositories)
        and set(reports) == set(repositories),
        "cluster repository report inventory differs",
    )
    rows = []
    for name in sorted(repositories):
        suite = suites[name]
        require(
            isinstance(suite, dict) and suite.get("repository_commit") == repositories[name],
            "cluster repository suite commit differs",
        )
        rows.extend(
            (name, task_id, family_id, delta)
            for task_id, family_id, _category, delta in paired_query_family_rows(
                suite, reports[name], baseline, candidate, require_graded=True
            )
        )
    return repository_cluster_ci(rows, release_digest, repositories, repository_strata)


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
    # This legacy metric treats candidate spans as retrieved content. A file
    # candidate's full-file proof span is an identity, not returned context.
    require(
        all(
            capture.get("execution_profile", {}).get("policy")
            not in query_plan_contract.FILE_PAIR_POLICIES
            for capture in run.get("captures", {}).values()
        ),
        "code_search_file requires file-judgment diagnostics; context metrics are undefined",
    )
    require(
        declared_evaluation_contract(suite.get("tasks", [])) is None,
        "evaluation_contract requires file/symbol judgment diagnostics, not context scoring",
    )
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
    prefix_diagnostics = suite.get("diagnostic_policy") == OBSERVED_PREFIX_DIAGNOSTIC_POLICY
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
    independent = judgment_diagnostics(suite, run, results, eval_tasks, baseline, candidate)
    if independent is not None:
        output["judgment_metrics"] = independent
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
            if prefix_diagnostics:
                capture_id = run["route_provenance"][route]["capture_id"]
                indexed_authority = (
                    run.get("span_accounting_version") == 1
                    and run["captures"][capture_id]["system"] == "quanta"
                    and status in SCORED_STATUSES
                )
                row["query_intent_claim"] = task.get("query_intent", "not_declared")
                row["label_review_claim"] = task.get("label_review", {"assessment": "not_declared"})
                if "source_oracle" in task:
                    row["source_oracle_claim"] = task["source_oracle"]
                row["observed_prefix"] = observed_prefix_diagnostics(
                    candidates,
                    labels,
                    status=status,
                    declared_top_k=declared_top_k,
                    indexed_span_authority=indexed_authority,
                )
            rows.append(row)
    output["per_query"] = rows
    return output


def no_answer_diagnostics(
    eval_tasks: dict[str, dict[str, Any]],
    results: dict[tuple[str, str], dict[str, Any]],
    route: str,
) -> dict[str, Any]:
    """Count nonempty results without assuming an absent label forbids content hits."""
    task_ids = sorted(task_id for task_id, task in eval_tasks.items() if not task["answerable"])
    status_counts: dict[str, int] = {}
    nonempty_results = 0
    for task_id in task_ids:
        result = results[(task_id, route)]
        status = _result_status(result)
        status_counts[status] = status_counts.get(status, 0) + 1
        nonempty_results += bool(_ordered_candidates(result))
    abstained = status_counts.get("abstained", 0)
    return {
        "task_ids": task_ids,
        "sample_count": len(task_ids),
        "reference_contracts": diagnostic_reference_contracts(
            {task_id: eval_tasks[task_id] for task_id in task_ids}
        ),
        "abstained": abstained,
        "abstention_rate": abstained / len(task_ids) if task_ids else NOT_APPLICABLE,
        "nonempty_results": nonempty_results,
        "nonempty_result_rate": nonempty_results / len(task_ids) if task_ids else NOT_APPLICABLE,
        "status_counts": status_counts,
    }


def diagnostic_reference_contracts(eval_tasks: dict[str, dict[str, Any]]) -> list[dict[str, str]]:
    """Expose the labels' source and intent without equating them to search semantics."""
    contracts = set()
    for task in eval_tasks.values():
        oracle = task.get("source_oracle") or {}
        evaluation = task.get("evaluation_contract") or {}
        contracts.add(
            (
                oracle.get("contract", "not_declared"),
                oracle.get("unit", evaluation.get("gold_unit", "not_declared")),
                task.get("query_intent", "not_declared"),
            )
        )
    return [
        {"source_oracle_contract": contract, "gold_unit": unit, "query_intent": intent}
        for contract, unit, intent in sorted(contracts)
    ]


def evaluate_diagnostic(
    suite: dict[str, Any], pack: dict[str, Any], run: dict[str, Any]
) -> dict[str, Any]:
    """Score one recorded route without inventing a paired quality comparison."""
    routes = suite["routes"]
    require(len(routes) == 1, "single-route diagnostic requires exactly one route")
    eval_tasks = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
    require(
        any(
            "file_judgments" in task or "declaration_judgments" in task
            for task in eval_tasks.values()
        ),
        "single-route diagnostic requires independent judgments",
    )
    results = {(row["task_id"], row["route"]): row for row in run["results"]}
    independent = judgment_diagnostics(suite, run, results, eval_tasks, routes[0], None)
    require(independent is not None, "independent judgment diagnostics unavailable")
    output = {
        "schema_version": SCHEMA_VERSION,
        "report_scope": "single_route_independent_judgment_diagnostic_v1",
        "status": "diagnostic_unqualified",
        "qualification": NOT_APPLICABLE,
        "suite_id": suite["suite_id"],
        "suite_commitment_sha256": digest(canonical(suite)),
        "query_pack_sha256": digest(canonical(pack)),
        "runner_record_sha256": digest(canonical(run)),
        "repository_commit": suite["repository_commit"],
        "comparison_contract": run["comparison_contract"],
        "route": routes[0],
        "route_provenance": run["route_provenance"],
        "captures": run["captures"],
        "runner": run["runner"],
        "selected_eval_tasks": len(eval_tasks),
        "reference_contracts": diagnostic_reference_contracts(eval_tasks),
        "no_answer": no_answer_diagnostics(eval_tasks, results, routes[0]),
        "paired_comparison": NOT_APPLICABLE,
        "quality_delta_gate": NOT_APPLICABLE,
        "judgment_metrics": independent,
    }
    evaluation_contract = declared_evaluation_contract(suite["tasks"])
    if evaluation_contract is not None:
        output["evaluation_contract"] = evaluation_contract
    return output


def evaluate_paired_file_diagnostic(
    suite: dict[str, Any],
    pack: dict[str, Any],
    run: dict[str, Any],
    baseline: str,
    candidate: str,
) -> dict[str, Any]:
    """Compare two source-bound distinct-file routes without context-span metrics."""
    require(
        set(suite["routes"]) == {baseline, candidate} and baseline != candidate,
        "paired file diagnostic requires exactly the two declared routes",
    )
    eval_tasks = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
    require(
        bool(eval_tasks) and all("file_judgments" in task for task in eval_tasks.values()),
        "paired file diagnostic requires independent file judgments on every eval task",
    )
    results = {(row["task_id"], row["route"]): row for row in run["results"]}
    for route in (baseline, candidate):
        require(
            all(
                results[(task_id, route)].get("rank_unit") == "distinct_file"
                for task_id in eval_tasks
            ),
            f"paired file diagnostic requires distinct-file results: {route}",
        )
    independent = judgment_diagnostics(suite, run, results, eval_tasks, baseline, candidate)
    require(
        independent is not None and "file_judgments" in independent,
        "paired file judgments are unavailable",
    )
    output = {
        "schema_version": SCHEMA_VERSION,
        "report_scope": "paired_independent_file_judgment_diagnostic_v1",
        "status": "diagnostic_unqualified",
        "qualification": NOT_APPLICABLE,
        "suite_id": suite["suite_id"],
        "suite_commitment_sha256": digest(canonical(suite)),
        "query_pack_sha256": digest(canonical(pack)),
        "runner_record_sha256": digest(canonical(run)),
        "repository_commit": suite["repository_commit"],
        "reference_contracts": diagnostic_reference_contracts(eval_tasks),
        "comparison_contract": run["comparison_contract"],
        "baseline_route": baseline,
        "candidate_route": candidate,
        "route_provenance": run["route_provenance"],
        "captures": run["captures"],
        "judgment_metrics": {"file_judgments": independent["file_judgments"]},
        "no_answer": {
            "routes": {
                route: no_answer_diagnostics(eval_tasks, results, route)
                for route in (baseline, candidate)
            }
        },
        "quality_delta_gate": NOT_APPLICABLE,
    }
    evaluation_contract = declared_evaluation_contract(suite["tasks"])
    if evaluation_contract is not None:
        output["evaluation_contract"] = evaluation_contract
    return output


def complete_scored_file_rows(
    suite: dict[str, Any],
    pack: dict[str, Any],
    run: dict[str, Any],
    baseline: str,
    candidate: str,
) -> list[tuple[str, float, float]]:
    """Require complete, scored file observations before any qualified use.

    The caller must first validate source, suite, pack and runner evidence.
    This selects paired rows only; it does not grant a quality or product claim.
    """
    report = evaluate_paired_file_diagnostic(suite, pack, run, baseline, candidate)
    tasks = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
    answerable = sorted(task_id for task_id, task in tasks.items() if task["answerable"])
    require(bool(answerable), "complete scored file comparison needs positive tasks")
    require(
        all(
            task.get("judgment_policy") in (SOURCE_ORACLE_JUDGMENT_POLICY, COMPLETE_JUDGMENT_POLICY)
            and (
                "source_oracle" in task
                or task.get("label_review", {}).get("assessment") in LABEL_REVIEW_ASSESSMENTS[1:]
            )
            for task in tasks.values()
        ),
        "complete scored file comparison lacks authoritative labels",
    )
    file_metrics = report["judgment_metrics"]["file_judgments"]
    for route, evidence in (
        (baseline, "semble_bm25_score_v1"),
        (candidate, "native_sdk_score_v1"),
    ):
        route_metrics = file_metrics["routes"][route]
        require(
            route_metrics["rank_metric_interpretation"]
            in ("scored_ranking", "scored_ranking_native_ties")
            and route_metrics["score_evidence"] == evidence
            and route_metrics["eligible_task_ids"] == answerable
            and not route_metrics["excluded"],
            f"complete scored file comparison lacks ordered, judged {route} rows",
        )
    comparison = file_metrics["comparison"]
    require(
        comparison["eligible_task_ids"] == answerable
        and comparison["sample_count"] == len(answerable),
        "complete scored file comparison lacks paired answerable coverage",
    )
    scores = {
        (row["task_id"], row["route"]): row["scores"]["ndcg_at_10"]
        for row in file_metrics["per_query"]
        if row["eligible"]
    }
    return [
        (task_id, scores[task_id, baseline], scores[task_id, candidate]) for task_id in answerable
    ]


def evaluate_complete_scored_file_evidence(
    suite: dict[str, Any],
    pack: dict[str, Any],
    run: dict[str, Any],
    baseline: str,
    candidate: str,
) -> dict[str, Any]:
    """Expose a fully paired file metric without claiming qualification."""
    rows = complete_scored_file_rows(suite, pack, run, baseline, candidate)
    eval_tasks = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
    results = {(row["task_id"], row["route"]): row for row in run["results"]}
    deltas = [after - before for _task_id, before, after in rows]
    negative_ids = sorted(task_id for task_id, task in eval_tasks.items() if not task["answerable"])
    # Positive-only cohorts measure ranked retrieval, not abstention. The
    # existing summary represents zero controls as not applicable, never zero.
    negative_status = {
        (task_id, route): _result_status(results[task_id, route])
        for task_id in negative_ids
        for route in (baseline, candidate)
    }
    require(
        all(status in (*SCORED_STATUSES, "abstained") for status in negative_status.values()),
        "complete scored file evidence has failed no-answer observations",
    )
    negative_rows = [
        (
            task_id,
            eval_tasks[task_id],
            float(negative_status[task_id, candidate] == "abstained")
            - float(negative_status[task_id, baseline] == "abstained"),
        )
        for task_id in negative_ids
    ]
    negative_evidence = no_answer_delta_summary(negative_rows, suite["repository_commit"])
    positive_rows = [
        (task_id, eval_tasks[task_id], delta)
        for (task_id, _before, _after), delta in zip(rows, deltas, strict=True)
    ]
    primary_delta = math.fsum(deltas) / len(deltas)
    wins = sum(delta > 0 for delta in deltas)
    losses = sum(delta < 0 for delta in deltas)
    per_query = []
    for task_id, before, after in rows:
        per_query.extend(
            [
                {"task_id": task_id, "route": baseline, "file_ndcg_at_10": before},
                {"task_id": task_id, "route": candidate, "file_ndcg_at_10": after},
            ]
        )
    for task_id in negative_ids:
        per_query.extend(
            {
                "task_id": task_id,
                "route": route,
                "status": negative_status[task_id, route],
            }
            for route in (baseline, candidate)
        )
    return {
        "schema_version": SCHEMA_VERSION,
        "report_scope": "paired_complete_scored_file_evidence_v1",
        "status": "evidence_unqualified",
        "rank_metric_version": "file-judgments-complete-v1",
        "graded": True,
        "suite_id": suite["suite_id"],
        "suite_commitment_sha256": digest(canonical(suite)),
        "query_pack_sha256": digest(canonical(pack)),
        "runner_record_sha256": digest(canonical(run)),
        "repository_commit": suite["repository_commit"],
        "rank_metrics": {
            "comparison": {
                "baseline": baseline,
                "candidate": candidate,
                "primary_metric": "file_ndcg_at_10",
                "sample_count": len(rows),
                "primary_delta": primary_delta,
                "paired_wins": wins,
                "paired_losses": losses,
                "paired_ties": len(rows) - wins - losses,
                "primary_delta_ci_95": mean_ci(
                    deltas,
                    [
                        (task_id, str(eval_tasks[task_id].get("category", "uncategorized")))
                        for task_id, _before, _after in rows
                    ],
                ),
                "stratified_primary_delta": stratified_delta_summary(
                    positive_rows, suite["repository_commit"]
                ),
                "no_answer_abstention_delta": negative_evidence,
            }
        },
        "per_query": sorted(per_query, key=lambda row: (row["task_id"], row["route"])),
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    for command in ("freeze", "evaluate", "evaluate-diagnostic"):
        child = sub.add_parser(command)
        child.add_argument("--repo", type=Path, required=True)
        child.add_argument("--suite", type=Path, required=True)
        child.add_argument("--output", type=Path)
        if command == "evaluate":
            child.add_argument("--runner", type=Path, required=True)
            child.add_argument("--baseline-route", required=True)
            child.add_argument("--candidate-route", required=True)
        if command == "evaluate-diagnostic":
            child.add_argument("--runner", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "freeze":
            _, value, _ = validate_suite(args.repo.resolve(), read_json(args.suite))
        elif args.command == "evaluate":
            suite, pack, run = load_evidence(args.repo.resolve(), args.suite, args.runner)
            value = evaluate(
                suite, pack, run, args.baseline_route, args.candidate_route, strict_k=True
            )
        else:
            suite, pack, run = load_evidence(args.repo.resolve(), args.suite, args.runner)
            value = evaluate_diagnostic(suite, pack, run)
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
