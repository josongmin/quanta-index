#!/usr/bin/env python3
"""Evaluate externally recorded retrieval routes against frozen repository labels.

The runner consumes only the output of ``freeze``. Gold labels are read only by
``evaluate``. Hashes and a clean, pinned Git checkout bind every scored block
to the repository contents; the evaluator never generates candidate results.

Schema v2 (current) adds: optional graded relevance (0-3) and category per
task, an optional canonical path+SHA file-universe allowlist, per-route
provenance, runner blinding (isolated|attested) with isolation method and
access-block log, typed non-success result statuses, measured timings with
finite checks, and tokenizer/budget version binding. Schema v1 suites and
runner records remain readable for migration; v1 requires both answerable and
no-gold eval tasks while v2 permits an all-answerable external suite (the
absent stratum reports ``not_applicable`` instead of invented tasks).

Freeze output never contains gold spans, grades, answerability bits, or train
tasks. Score computation depends only on recorded candidates and statuses,
never on runner identity or provenance strings.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path, PurePosixPath
from typing import Any

SCHEMA_VERSION = 2
SUITE_VERSIONS = (1, 2)
RUNNER_VERSIONS = (1, 2)
BUDGETS = (2000, 4000, 8000, 16000)
TOKENIZER = "qi-regex-v1"
TOKENIZER_BUDGET_VERSION = "qb-v1"
# Explicit ASCII ranges keep token accounting stable across Python's Unicode
# database revisions. Non-ASCII code points each count as one benchmark unit.
TOKEN_RE = re.compile(r"[A-Za-z0-9_]+|[^\x00-\x20]")
HEX64_RE = re.compile(r"[0-9a-f]{64}\Z")
COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")

RECALL_KS = (1, 5, 10, 20)
MRR_K = 10
NDCG_K = 10
RESULT_STATUSES = ("success", "abstained", "capped", "error", "timeout", "unavailable")
NON_SUCCESS_EMPTY = ("abstained", "error", "timeout", "unavailable")
SCORED_STATUSES = ("success", "capped")
BLINDING_VALUES = ("isolated", "attested")
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
    require(type(value) is int and 0 <= value <= 3, f"{where} grade must be an integer 0-3")
    return value


def finite_timing(value: Any, where: str) -> float:
    require(
        (type(value) is int or type(value) is float)
        and math.isfinite(float(value))
        and float(value) >= 0,
        f"{where} timing must be a finite number >= 0",
    )
    return float(value)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False
    ).encode("utf-8")


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
    require(repo.is_dir(), f"repository checkout missing: {repo}")
    root = Path(git(repo, "rev-parse", "--show-toplevel")).resolve()
    require(root == repo.resolve(), "--repo must name the checkout root")
    require(git(root, "rev-parse", "HEAD") == commit, "checkout HEAD differs from frozen commit")
    require(
        not git(root, "status", "--porcelain", "--untracked-files=all"),
        "checkout has tracked or untracked changes",
    )
    return root


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
        self.blocks: dict[tuple[str, int, int], tuple[str, int]] = {}

    def file(self, name: str) -> tuple[bytes, list[bytes], str]:
        if name not in self.files:
            absolute = safe_path(self, name)
            raw = absolute.read_bytes()
            self.files[name] = (raw, raw.splitlines(keepends=True), digest(raw))
        return self.files[name]

    def block_data(self, name: str, start: int, end: int) -> tuple[str, int]:
        key = (name, start, end)
        if key not in self.blocks:
            _, lines, _ = self.file(name)
            require(end <= len(lines), f"block spans beyond EOF: {name}:{start}-{end}")
            selected = b"".join(lines[start - 1 : end])
            try:
                text = selected.decode("utf-8")
            except UnicodeDecodeError as exc:
                raise EvidenceError(
                    f"block is not UTF-8 source text: {name}:{start}-{end}"
                ) from exc
            count = len(TOKEN_RE.findall(text))
            require(count > 0, f"block contains no retrievable tokens: {name}:{start}-{end}")
            self.blocks[key] = (digest(selected), count)
        return self.blocks[key]


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
    require_rank: bool = False,
) -> dict[str, Any]:
    required = ["path", "start_line", "end_line", "file_sha256", "block_sha256"]
    if candidate:
        required.append("tokens")
    if require_rank:
        required.append("rank")
    optional = ["grade"] if (allow_grade and not candidate) else []
    if optional:
        item = object_keys_optional(value, required, optional, where)
    else:
        item = object_keys(value, required, where)
    path = string(item["path"], where + ".path")
    start = positive_int(item["start_line"], where + ".start_line")
    end = positive_int(item["end_line"], where + ".end_line")
    require(start <= end, where + " has inverted line span")
    _, lines, file_digest = source.file(path)
    require(end <= len(lines), where + " spans beyond EOF")
    require(
        sha(item["file_sha256"], where + ".file_sha256") == file_digest,
        where + " file hash mismatch",
    )
    if universe is not None:
        require(path in universe, where + f" file excluded from file universe: {path}")
    block_digest, count = source.block_data(path, start, end)
    require(
        sha(item["block_sha256"], where + ".block_sha256") == block_digest,
        where + " block hash mismatch",
    )
    if candidate:
        require(
            positive_int(item["tokens"], where + ".tokens") == count,
            where + " token count mismatch",
        )
    if require_rank:
        positive_int(item["rank"], where + ".rank")
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


def _check_split_leakage(labels_by_split: dict[str, set[tuple[str, int, int]]]) -> None:
    spans_by_path: dict[str, list[tuple[int, int, str]]] = {}
    for split, labels in labels_by_split.items():
        for path, start, end in labels:
            spans_by_path.setdefault(path, []).append((start, end, split))
    for spans in spans_by_path.values():
        furthest = {"train": 0, "eval": 0}
        for start, end, split in sorted(spans):
            other = "eval" if split == "train" else "train"
            require(furthest[other] < start, "gold label leakage across train/eval split")
            furthest[split] = max(furthest[split], end)


def _validate_suite_v1(
    repo: Path, payload: Any
) -> tuple[dict[str, Any], dict[str, Any], SourceSnapshot]:
    suite = object_keys(
        payload, ["schema_version", "suite_id", "repository_commit", "routes", "tasks"], "suite"
    )
    require(type(suite["schema_version"]) is int and suite["schema_version"] == 1, "unsupported suite schema")
    string(suite["suite_id"], "suite_id")
    commit = suite["repository_commit"]
    require(
        isinstance(commit, str) and bool(COMMIT_RE.fullmatch(commit)),
        "repository_commit must be a full lowercase Git SHA",
    )
    source = SourceSnapshot(repo, commit)
    routes = suite["routes"]
    require(
        isinstance(routes, list) and len(routes) >= 2,
        "suite requires at least two routes for ablation",
    )
    for route in routes:
        string(route, "route")
    require(len(set(routes)) == len(routes), "duplicate route")
    tasks = suite["tasks"]
    require(isinstance(tasks, list) and bool(tasks), "suite requires tasks")
    seen_ids = set()
    seen_queries = set()
    eval_count = 0
    gold_count = 0
    no_gold_count = 0
    labels_by_split: dict[str, set[tuple[str, int, int]]] = {"train": set(), "eval": set()}
    for raw in tasks:
        task = object_keys(
            raw, ["task_id", "split", "query", "query_sha256", "answerable", "gold"], "task"
        )
        task_id = string(task["task_id"], "task_id")
        require(task_id not in seen_ids, "duplicate task_id: " + task_id)
        seen_ids.add(task_id)
        require(task["split"] in ("train", "eval"), "invalid task split: " + task_id)
        query = string(task["query"], "query")
        query_hash = sha(task["query_sha256"], "query_sha256")
        require(digest(query.encode("utf-8")) == query_hash, "query hash mismatch: " + task_id)
        require(
            query_hash not in seen_queries, "query leakage/duplication across tasks: " + task_id
        )
        seen_queries.add(query_hash)
        require(type(task["answerable"]) is bool, "answerable must be boolean: " + task_id)
        labels = task["gold"]
        require(isinstance(labels, list), "gold must be a list: " + task_id)
        require(task["answerable"] == bool(labels), "answerable/gold mismatch: " + task_id)
        seen_labels = set()
        for label in labels:
            block(source, label, "gold label for " + task_id, candidate=False)
            key = (label["path"], label["start_line"], label["end_line"])
            require(key not in seen_labels, "duplicate gold label: " + task_id)
            seen_labels.add(key)
            labels_by_split[task["split"]].add(key)
        if task["split"] == "eval":
            eval_count += 1
            gold_count += bool(labels)
            no_gold_count += not bool(labels)
    require(
        eval_count > 0 and gold_count > 0 and no_gold_count > 0,
        "eval split requires answerable and no-gold tasks",
    )
    _check_split_leakage(labels_by_split)
    blinded = {
        "schema_version": 1,
        "suite_id": suite["suite_id"],
        "suite_commitment_sha256": digest(canonical(suite)),
        "repository_commit": commit,
        "tokenizer": TOKENIZER,
        "routes": routes,
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
    return suite, blinded, source


def _validate_suite_v2(
    repo: Path, payload: Any
) -> tuple[dict[str, Any], dict[str, Any], SourceSnapshot]:
    suite = object_keys_optional(
        payload,
        ["schema_version", "suite_id", "repository_commit", "routes", "tasks"],
        ["file_universe"],
        "suite",
    )
    require(type(suite["schema_version"]) is int and suite["schema_version"] == 2, "unsupported suite schema")
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
    universe: set[str] | None = None
    ordered_universe: list[dict[str, str]] = []
    if "file_universe" in suite:
        entries, ordered_universe = validate_file_universe(source, suite["file_universe"])
        universe = set(entries)
    tasks = suite["tasks"]
    require(isinstance(tasks, list) and bool(tasks), "suite requires tasks")
    seen_ids = set()
    seen_queries = set()
    eval_count = 0
    labels_by_split: dict[str, set[tuple[str, int, int]]] = {"train": set(), "eval": set()}
    for raw in tasks:
        task = object_keys_optional(
            raw,
            ["task_id", "split", "query", "query_sha256", "answerable", "gold"],
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
    _check_split_leakage(labels_by_split)
    blinded = {
        "schema_version": 2,
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
    return suite, blinded, source


def validate_suite(
    repo: Path, payload: Any
) -> tuple[dict[str, Any], dict[str, Any], SourceSnapshot]:
    require(isinstance(payload, dict), "suite must be an object")
    version = payload.get("schema_version")
    require(type(version) is int and version in SUITE_VERSIONS, "unsupported suite schema")
    if version == 1:
        return _validate_suite_v1(repo, payload)
    return _validate_suite_v2(repo, payload)


def _load_run_v1(
    run: dict[str, Any],
    pack: dict[str, Any],
    suite: dict[str, Any],
    source: SourceSnapshot,
) -> dict[str, Any]:
    require(
        type(run["schema_version"]) is int and run["schema_version"] == 1,
        "unsupported runner schema",
    )
    require(
        sha(run["query_pack_sha256"], "query_pack_sha256") == digest(canonical(pack)),
        "runner query pack hash mismatch",
    )
    runner = object_keys(
        run["runner"], ["name", "revision", "run_id", "tokenizer", "gold_access"], "runner"
    )
    for key in ("name", "revision", "run_id"):
        string(runner[key], "runner." + key)
    require(runner["tokenizer"] == TOKENIZER, "runner tokenizer mismatch")
    require(
        runner["gold_access"] is False, "runner attests gold access or lacks no-gold attestation"
    )
    results = run["results"]
    require(isinstance(results, list), "results must be a list")
    tasks = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
    expected = {(task_id, route) for task_id in tasks for route in suite["routes"]}
    found = set()
    for raw in results:
        result = object_keys(raw, ["task_id", "route", "abstain", "candidates"], "result")
        key = (string(result["task_id"], "result.task_id"), string(result["route"], "result.route"))
        require(key in expected and key not in found, f"unexpected/duplicate task route: {key}")
        found.add(key)
        require(type(result["abstain"]) is bool, f"abstain must be boolean: {key}")
        candidates = result["candidates"]
        require(isinstance(candidates, list), f"candidates must be a list: {key}")
        require(
            not result["abstain"] or not candidates, f"abstention cannot contain candidates: {key}"
        )
        require(result["abstain"] or bool(candidates), f"empty non-abstaining result: {key}")
        seen_blocks = set()
        for candidate in candidates:
            block(source, candidate, f"candidate for {key}", candidate=True)
            span = (candidate["path"], candidate["start_line"], candidate["end_line"])
            require(span not in seen_blocks, f"duplicate candidate block: {key}")
            seen_blocks.add(span)
    require(found == expected, f"missing task route evidence: {sorted(expected - found)}")
    return run


def _load_run_v2(
    run: dict[str, Any],
    pack: dict[str, Any],
    suite: dict[str, Any],
    source: SourceSnapshot,
) -> dict[str, Any]:
    require(
        type(run["schema_version"]) is int and run["schema_version"] == 2,
        "unsupported runner schema",
    )
    require(
        sha(run["query_pack_sha256"], "query_pack_sha256") == digest(canonical(pack)),
        "runner query pack hash mismatch",
    )
    runner = object_keys(
        run["runner"],
        [
            "name",
            "revision",
            "run_id",
            "tokenizer",
            "tokenizer_budget_version",
            "gold_access",
            "blinding",
            "isolation_method",
            "access_block_log",
        ],
        "runner",
    )
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
    provenance = run["route_provenance"]
    require(isinstance(provenance, dict), "route_provenance must be an object")
    require(
        set(provenance) == set(suite["routes"]),
        f"route_provenance has missing/unknown routes: {sorted(set(provenance) ^ set(suite['routes']))}",
    )
    for route, entry in provenance.items():
        item = object_keys(entry, ["system", "model", "model_revision"], f"route_provenance.{route}")
        for key in ("system", "model", "model_revision"):
            string(item[key], f"route_provenance.{route}.{key}")
    universe: set[str] | None = None
    if "file_universe" in suite:
        universe = {entry["path"] for entry in suite["file_universe"]}
    results = run["results"]
    require(isinstance(results, list), "results must be a list")
    tasks = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
    expected = {(task_id, route) for task_id in tasks for route in suite["routes"]}
    found = set()
    for raw in results:
        result = object_keys(
            raw, ["task_id", "route", "status", "candidates", "timings", "error"], "result"
        )
        key = (string(result["task_id"], "result.task_id"), string(result["route"], "result.route"))
        require(key in expected and key not in found, f"unexpected/duplicate task route: {key}")
        found.add(key)
        status = result["status"]
        require(status in RESULT_STATUSES, f"unknown result status for {key}: {status!r}")
        candidates = result["candidates"]
        require(isinstance(candidates, list), f"candidates must be a list: {key}")
        timings = object_keys(result["timings"], ["query_latency_ms"], f"timings for {key}")
        finite_timing(timings["query_latency_ms"], f"timings for {key}")
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
        for index, candidate in enumerate(candidates, start=1):
            block(
                source,
                candidate,
                f"candidate for {key}",
                candidate=True,
                universe=universe,
                require_rank=True,
            )
            require(
                candidate["rank"] == index,
                f"duplicate/non-sequential candidate rank for {key}: expected {index}",
            )
    require(found == expected, f"missing task route evidence: {sorted(expected - found)}")
    return run


def load_evidence(
    repo: Path, suite_path: Path, runner_path: Path
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    suite, pack, source = validate_suite(repo, read_json(suite_path))
    payload = read_json(runner_path)
    version = payload.get("schema_version") if isinstance(payload, dict) else None
    require(type(version) is int and version in RUNNER_VERSIONS, "unsupported runner schema")
    require(
        version == suite["schema_version"],
        "suite/runner schema version mismatch",
    )
    if version == 1:
        run = object_keys(
            payload,
            ["schema_version", "query_pack_sha256", "runner", "results"],
            "runner record",
        )
        _load_run_v1(run, pack, suite, source)
    else:
        run = object_keys(
            payload,
            ["schema_version", "query_pack_sha256", "runner", "route_provenance", "results"],
            "runner record",
        )
        _load_run_v2(run, pack, suite, source)
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
    return (
        candidate["path"] == label["path"]
        and candidate["start_line"] <= label["start_line"]
        and candidate["end_line"] >= label["end_line"]
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


def ndcg_at_k(candidates: list[dict[str, Any]], labels: list[dict[str, Any]], k: int) -> float:
    gains = []
    credited_labels: set[int] = set()
    for item in candidates[:k]:
        rel = 0
        for index, label in enumerate(labels):
            if index not in credited_labels and covers(item, label):
                rel = max(rel, int(label.get("grade", 1)))
                credited_labels.add(index)
        gains.append(2**rel - 1)
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


def mean_ci(deltas: list[float]) -> dict[str, Any]:
    n = len(deltas)
    if n < MIN_CI_SAMPLE:
        return {
            "status": NOT_APPLICABLE,
            "reason": "insufficient_sample",
            "sample_count": n,
            "min_sample": MIN_CI_SAMPLE,
        }
    mean = sum(deltas) / n
    if n == 1:
        return {
            "sample_count": n,
            "method": "normal_approx",
            "mean": mean,
            "lower_95": mean,
            "upper_95": mean,
        }
    var = sum((d - mean) ** 2 for d in deltas) / (n - 1)
    se = math.sqrt(var) / math.sqrt(n)
    margin = 1.96 * se
    return {
        "sample_count": n,
        "method": "normal_approx",
        "mean": mean,
        "lower_95": mean - margin,
        "upper_95": mean + margin,
    }


def _result_status(result: dict[str, Any], version: int) -> str:
    if version == 1:
        return "abstained" if result["abstain"] else "success"
    return str(result["status"])


def _result_latency(result: dict[str, Any], version: int) -> float | None:
    if version == 1:
        return None
    return float(result["timings"]["query_latency_ms"])


def _result_error_code(result: dict[str, Any], version: int) -> str | None:
    if version == 1 or result.get("error") is None:
        return None
    return str(result["error"]["code"])


def _ordered_candidates(result: dict[str, Any], version: int) -> list[dict[str, Any]]:
    candidates = list(result["candidates"])
    if version == 2:
        candidates.sort(key=lambda c: int(c["rank"]))
    return candidates


def evaluate(
    suite: dict[str, Any], pack: dict[str, Any], run: dict[str, Any], baseline: str, candidate: str
) -> dict[str, Any]:
    require(
        baseline in suite["routes"] and candidate in suite["routes"] and baseline != candidate,
        "comparison routes must be distinct registered routes",
    )
    version = int(suite["schema_version"])
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
    if "file_universe" in suite:
        ordered = sorted(suite["file_universe"], key=lambda e: str(e["path"]))
        universe_digest: str | None = digest(canonical(ordered))
    else:
        universe_digest = None
    output: dict[str, Any] = {
        "schema_version": version,
        "suite_id": suite["suite_id"],
        "suite_commitment_sha256": digest(canonical(suite)),
        "query_pack_sha256": digest(canonical(pack)),
        "runner_record_sha256": digest(canonical(run)),
        "repository_commit": suite["repository_commit"],
        "runner": run["runner"],
        "tokenizer": TOKENIZER,
        "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
        "file_universe_digest": universe_digest,
        "primary_metric": primary_metric,
        "graded": graded,
        "answerable_tasks": len(answerable_ids),
        "no_gold_tasks": len(no_gold_ids),
        "sample_count": len(task_ids),
        "budgets": {},
    }
    if version == 2:
        output["rank_metric_version"] = "rb-rank-v2-first-coverage"
        output["route_provenance"] = run["route_provenance"]
        output["blinding"] = run["runner"]["blinding"]
    # Budgeted BCY view (v1-compatible numbers, NA-aware for v2 strata).
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
                status = _result_status(result, version)
                status_counts[status] = status_counts.get(status, 0) + 1
                latency = _result_latency(result, version)
                if latency is not None:
                    latencies.append(latency)
                candidates = _ordered_candidates(result, version)
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
        wins = sum(
            successes[candidate][t] and not successes[baseline][t] for t in task_ids
        )
        losses = sum(
            successes[baseline][t] and not successes[candidate][t] for t in task_ids
        )
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
                "success_delta_ci_95": mean_ci(binary_deltas),
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
            status = _result_status(result, version)
            status_counts[status] = status_counts.get(status, 0) + 1
            latency = _result_latency(result, version)
            if latency is not None:
                latencies.append(latency)
            if task_id not in per_task_primary:
                continue
            labels = eval_tasks[task_id]["gold"]
            if status in SCORED_STATUSES:
                candidates = _ordered_candidates(result, version)
                collapsed = collapse_by_file(candidates)
                chunk_vals = {f"recall_at_{k}": recall_at_k(candidates, labels, k) for k in RECALL_KS}
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
            rank_delta[key] = NOT_APPLICABLE if (a == NOT_APPLICABLE or b == NOT_APPLICABLE) else (b - a)
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
            "primary_delta_ci_95": mean_ci(primary_deltas),
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
        }
    output["rank_metrics"] = {"routes": rank_routes, "comparison": rank_comparison}
    # Per-query rows in deterministic task/route order.
    rows = []
    for task_id in task_ids:
        task = eval_tasks[task_id]
        labels = task["gold"]
        for route in sorted(routes):
            result = results[(task_id, route)]
            status = _result_status(result, version)
            candidates = _ordered_candidates(result, version)
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
                    "error_code": _result_error_code(result, version),
                    "query_latency_ms": _result_latency(result, version),
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
                    "error_code": None,
                    "query_latency_ms": _result_latency(result, version),
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
                    "error_code": _result_error_code(result, version),
                    "query_latency_ms": _result_latency(result, version),
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
            value = evaluate(suite, pack, run, args.baseline_route, args.candidate_route)
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
