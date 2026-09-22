#!/usr/bin/env python3
"""Evaluate externally recorded retrieval routes against frozen repository labels.

The runner consumes only the output of ``freeze``. Gold labels are read only by
``evaluate``. Hashes and a clean, pinned Git checkout bind every scored block
to the repository contents; the evaluator never generates candidate results.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path, PurePosixPath
from typing import Any

SCHEMA_VERSION = 1
BUDGETS = (2000, 4000, 8000, 16000)
TOKENIZER = "qi-regex-v1"
# Explicit ASCII ranges keep token accounting stable across Python's Unicode
# database revisions. Non-ASCII code points each count as one benchmark unit.
TOKEN_RE = re.compile(r"[A-Za-z0-9_]+|[^\x00-\x20]")
HEX64_RE = re.compile(r"[0-9a-f]{64}\Z")
COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")


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


def block(source: SourceSnapshot, value: Any, where: str, *, candidate: bool) -> dict[str, Any]:
    keys = ["path", "start_line", "end_line", "file_sha256", "block_sha256"]
    if candidate:
        keys.append("tokens")
    item = object_keys(value, keys, where)
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
    return item


def validate_suite(
    repo: Path, payload: Any
) -> tuple[dict[str, Any], dict[str, Any], SourceSnapshot]:
    suite = object_keys(
        payload, ["schema_version", "suite_id", "repository_commit", "routes", "tasks"], "suite"
    )
    require(
        type(suite["schema_version"]) is int and suite["schema_version"] == SCHEMA_VERSION,
        "unsupported suite schema",
    )
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
    labels_by_split = {"train": set(), "eval": set()}
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
    blinded = {
        "schema_version": SCHEMA_VERSION,
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


def load_evidence(
    repo: Path, suite_path: Path, runner_path: Path
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    suite, pack, source = validate_suite(repo, read_json(suite_path))
    run = object_keys(
        read_json(runner_path),
        ["schema_version", "query_pack_sha256", "runner", "results"],
        "runner record",
    )
    require(
        type(run["schema_version"]) is int and run["schema_version"] == SCHEMA_VERSION,
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


def evaluate(
    suite: dict[str, Any], pack: dict[str, Any], run: dict[str, Any], baseline: str, candidate: str
) -> dict[str, Any]:
    require(
        baseline in suite["routes"] and candidate in suite["routes"] and baseline != candidate,
        "comparison routes must be distinct registered routes",
    )
    tasks = {task["task_id"]: task for task in suite["tasks"] if task["split"] == "eval"}
    results = {(row["task_id"], row["route"]): row for row in run["results"]}
    output: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "suite_id": suite["suite_id"],
        "suite_commitment_sha256": digest(canonical(suite)),
        "query_pack_sha256": digest(canonical(pack)),
        "runner_record_sha256": digest(canonical(run)),
        "repository_commit": suite["repository_commit"],
        "runner": run["runner"],
        "tokenizer": TOKENIZER,
        "budgets": {},
    }
    for budget in BUDGETS:
        per_route: dict[str, Any] = {}
        successes: dict[str, dict[str, bool]] = {}
        for route in suite["routes"]:
            answerable = 0
            no_gold = 0
            full = 0
            file_recall = 0.0
            block_recall = 0.0
            abstentions = 0
            consumed = 0
            outcomes: dict[str, bool] = {}
            for task_id, task in tasks.items():
                result = results[(task_id, route)]
                chosen, used = selected(result["candidates"], budget)
                consumed += used
                labels = task["gold"]
                if not labels:
                    no_gold += 1
                    success = result["abstain"]
                    abstentions += int(success)
                else:
                    answerable += 1
                    gold_files = {label["path"] for label in labels}
                    hit_files = gold_files.intersection({item["path"] for item in chosen})
                    file_recall += len(hit_files) / len(gold_files)
                    hit_blocks = sum(
                        any(covers(item, label) for item in chosen) for label in labels
                    )
                    block_recall += hit_blocks / len(labels)
                    success = hit_blocks == len(labels) and not result["abstain"]
                    full += int(success)
                outcomes[task_id] = success
            per_route[route] = {
                "answerable_tasks": answerable,
                "no_gold_tasks": no_gold,
                "bcy": full / answerable,
                "file_recall": file_recall / answerable,
                "block_recall": block_recall / answerable,
                "no_gold_abstention": abstentions / no_gold,
                "mean_context_tokens": consumed / len(tasks),
            }
            successes[route] = outcomes
        comparisons = {
            metric: per_route[candidate][metric] - per_route[baseline][metric]
            for metric in (
                "bcy",
                "file_recall",
                "block_recall",
                "no_gold_abstention",
                "mean_context_tokens",
            )
        }
        wins = sum(
            successes[candidate][task_id] and not successes[baseline][task_id] for task_id in tasks
        )
        losses = sum(
            successes[baseline][task_id] and not successes[candidate][task_id] for task_id in tasks
        )
        output["budgets"][str(budget)] = {
            "routes": per_route,
            "comparison": {
                "baseline": baseline,
                "candidate": candidate,
                "delta": comparisons,
                "paired_wins": wins,
                "paired_losses": losses,
                "paired_ties": len(tasks) - wins - losses,
            },
        }
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
