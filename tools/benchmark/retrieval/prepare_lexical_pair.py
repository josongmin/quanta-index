#!/usr/bin/env python3
"""Derive an exploratory bare-symbol lexical pair from frozen benchmark inputs.

The output is a new spec, not a qualification receipt. The suite and corpus
remain external; this tool refuses a query-only rewrite that changes labels.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

from tools.benchmark.retrieval import query_plan, run
from tools.benchmark.retrieval.evaluator import canonical, digest
from tools.benchmark.retrieval.lexical_file_comparison import (
    QUANTA_LEXICAL_ROUTE,
    SEMBLE_LEXICAL_ROUTE,
)

IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z_0-9]*\Z")


def _read(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"expected JSON object: {path}")
    return value


def _sha_file(path: Path) -> str:
    sha = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            sha.update(block)
    return sha.hexdigest()


def _tasks_by_id(value: dict, label: str) -> dict[str, dict]:
    tasks = value.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise ValueError(f"{label} lacks tasks")
    by_id = {task.get("task_id"): task for task in tasks if isinstance(task, dict)}
    if len(by_id) != len(tasks) or not all(isinstance(key, str) and key for key in by_id):
        raise ValueError(f"{label} has malformed or duplicate task IDs")
    return by_id


def build_spec(
    base: dict,
    original_suite: dict,
    suite: dict,
    pack: dict,
    *,
    suite_path: Path,
    pack_path: Path,
    output_root: Path,
    run_id: str,
) -> dict:
    original = _tasks_by_id(original_suite, "original suite")
    lexical = _tasks_by_id(suite, "lexical suite")
    blinded = _tasks_by_id(pack, "lexical query pack")
    if original.keys() != lexical.keys() or lexical.keys() != blinded.keys():
        raise ValueError("task IDs differ across the source, lexical suite and pack")
    if len(lexical) < 20:
        raise ValueError("lexical diagnostic requires at least 20 distinct tasks")
    for key in (
        "repository_commit",
        "file_universe_digest",
        "file_universe",
        "comparison_contract",
    ):
        if original_suite.get(key) != suite.get(key) or suite.get(key) != pack.get(key):
            raise ValueError(f"{key} differs across frozen inputs")
    if suite.get("routes") != [QUANTA_LEXICAL_ROUTE, SEMBLE_LEXICAL_ROUTE] or pack.get("routes") != suite["routes"]:
        raise ValueError("lexical suite must declare lexical and semble-lexical-only routes")
    if pack.get("suite_commitment_sha256") != digest(canonical(suite)):
        raise ValueError("query pack does not bind the lexical suite")
    for task_id, task in lexical.items():
        old = original[task_id]
        query = task.get("query")
        if not isinstance(query, str) or IDENTIFIER.fullmatch(query) is None:
            raise ValueError(f"{task_id} is not one bare identifier")
        if old == task or {k: v for k, v in old.items() if k not in ("query", "query_sha256")} != {
            k: v for k, v in task.items() if k not in ("query", "query_sha256")
        }:
            raise ValueError(f"{task_id} changed more than its query")
        if task.get("query_sha256") != hashlib.sha256(query.encode()).hexdigest():
            raise ValueError(f"{task_id} query digest is wrong")
        if blinded[task_id] != {
            "task_id": task_id,
            "query": query,
            "query_sha256": task["query_sha256"],
        }:
            raise ValueError(f"{task_id} blinded pack differs from suite")
    if base.get("scope") != "exploratory" or any(base.get("claims", {}).values()):
        raise ValueError("base spec must be exploratory with every qualification claim disabled")
    if output_root.exists() or output_root.with_name(output_root.name + ".staging").exists():
        raise ValueError("output root or staging path already exists")
    result = dict(base)
    result.update(
        suite=str(suite_path.resolve()),
        query_pack=str(pack_path.resolve()),
        output_root=str(output_root.resolve()),
        run_id=run_id,
        searchd_expected_sha256=_sha_file(Path(base["searchd_binary"])),
        routes=[QUANTA_LEXICAL_ROUTE],
        candidate_route=QUANTA_LEXICAL_ROUTE,
        baseline_route=SEMBLE_LEXICAL_ROUTE,
        semble_route=SEMBLE_LEXICAL_ROUTE,
        execution_profiles={
            "quanta": query_plan.execution_profile("native"),
            "semble": {
                "profile_id": "semble-lexical-only-v1",
                "mode": "lexical-only",
                "alpha": None,
                "rerank": "not_applicable",
            },
        },
    )
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-spec", type=Path, required=True)
    parser.add_argument("--suite", type=Path, required=True)
    parser.add_argument("--query-pack", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--spec-out", type=Path, required=True)
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()
    base = run.load_spec(args.base_spec)
    result = build_spec(
        base,
        _read(Path(base["suite"])),
        _read(args.suite),
        _read(args.query_pack),
        suite_path=args.suite,
        pack_path=args.query_pack,
        output_root=args.output_root,
        run_id=args.run_id,
    )
    if args.spec_out.exists():
        raise ValueError("spec output already exists")
    args.spec_out.parent.mkdir(parents=True, exist_ok=True)
    args.spec_out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    run.load_spec(args.spec_out)


if __name__ == "__main__":
    main()
