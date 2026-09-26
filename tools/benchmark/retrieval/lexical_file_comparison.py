#!/usr/bin/env python3
"""Validate one bare-symbol file-recall diagnostic across code-search products.

This intentionally does not compare native ranking or latency: the endpoints
have different ranking units and timing layers. Mechanical labels are not an
independent quality oracle.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

from tools.benchmark.retrieval.evaluator import canonical, digest
from tools.benchmark.retrieval.query_plan import execution_profile

PRODUCTS = ("sourcegraph", "opengrok", "cs")


def _read(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"expected object: {path}")
    return value


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _tasks(suite: dict, pack: dict) -> dict[str, tuple[str, list[str]]]:
    if pack.get("suite_commitment_sha256") != digest(canonical(suite)):
        raise ValueError("pack and suite commitment differ")
    if suite.get("repository_commit") != pack.get("repository_commit"):
        raise ValueError("pack and suite repository commits differ")
    pack_tasks = pack.get("tasks")
    suite_tasks = suite.get("tasks")
    if (
        not isinstance(pack_tasks, list)
        or not isinstance(suite_tasks, list)
        or len(pack_tasks) != len(suite_tasks)
        or len(pack_tasks) < 20
    ):
        raise ValueError("pack and suite task counts differ or are insufficient")
    expected: dict[str, tuple[str, list[str]]] = {}
    blinded: dict[str, tuple[str, str]] = {}
    for task in pack_tasks:
        if not isinstance(task, dict) or set(task) != {"task_id", "query", "query_sha256"}:
            raise ValueError("malformed blinded query task")
        task_id, query = task["task_id"], task["query"]
        if (
            not isinstance(task_id, str)
            or not task_id
            or not isinstance(query, str)
            or not query
            or task_id in blinded
        ):
            raise ValueError("duplicate or malformed blinded query")
        if task["query_sha256"] != hashlib.sha256(query.encode()).hexdigest():
            raise ValueError(f"{task_id} query digest differs")
        blinded[task_id] = query, task["query_sha256"]
    for task in suite_tasks:
        if not isinstance(task, dict):
            raise ValueError("malformed suite task")
        task_id = task.get("task_id")
        if (
            task_id not in blinded
            or task_id in expected
            or (task.get("query"), task.get("query_sha256")) != blinded[task_id]
        ):
            raise ValueError("suite and blinded query tasks differ")
        gold = task.get("gold")
        if not isinstance(gold, list) or not gold:
            raise ValueError("this diagnostic requires answerable file labels")
        paths = sorted({label["path"] for label in gold})
        expected[task_id] = task["query"], paths
    return expected


def product_result(product: str, path: Path, expected: dict[str, tuple[str, list[str]]]) -> dict:
    rows = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]
    selected = [row for row in rows if row.get("lane") == "symbol_only"]
    if len(selected) != len(expected):
        raise ValueError(f"{product}: incomplete symbol-only lane")
    seen: set[str] = set()
    hits = 0
    for row in selected:
        task_id = row.get("task_id")
        if task_id not in expected or task_id in seen:
            raise ValueError(f"{product}: missing or duplicate task")
        seen.add(task_id)
        query, gold = expected[task_id]
        if row.get("submitted_query") != query or row.get("gold_paths") != gold:
            raise ValueError(f"{product}: {task_id} query or gold differs")
        if product == "cs":
            if row.get("exit_code") != 0:
                raise ValueError(f"{product}: {task_id} failed process")
            paths = row.get("paths")
        else:
            if row.get("http_status") != 200 or row.get("error") is not None:
                raise ValueError(f"{product}: {task_id} failed request")
            if product == "opengrok" and row.get("field") != "full":
                raise ValueError(f"{product}: {task_id} used a non-full field")
            paths = row.get("file_paths_top_10")
        if not isinstance(paths, list) or len(paths) > 10 or len(paths) != len(set(paths)):
            raise ValueError(f"{product}: {task_id} malformed result paths")
        hit = bool(set(paths) & set(gold))
        if row.get("file_hit_at_10") is not hit:
            raise ValueError(f"{product}: {task_id} hit flag differs from paths")
        hits += hit
    return {
        "hits": hits,
        "tasks": len(expected),
        "file_recall_at_10": hits / len(expected),
        "raw_sha256": _sha(path),
    }


def pair_result(
    path: Path,
    lock_path: Path,
    native_path: Path,
    verdict_path: Path,
    pack: dict,
    suite: dict,
    task_count: int,
) -> dict:
    report = _read(path)
    lock, native, verdict = _read(lock_path), _read(native_path), _read(verdict_path)
    if verdict.get("states", {}).get("PAIR_VALID") != "pass":
        raise ValueError("pair verdict is not valid")
    profiles = lock.get("execution_profiles", {})
    if profiles.get("quanta") != execution_profile("native") or profiles.get("semble") != {
        "profile_id": "semble-lexical-only-v1",
        "mode": "lexical-only",
        "alpha": None,
        "rerank": "not_applicable",
    }:
        raise ValueError("pair execution profiles are not pure lexical")
    counts = native.get("lane_call_counts", {})
    events = native.get("execution_events")
    if (
        native.get("semble_profile") != "lexical-only"
        or native.get("rerank_applied") is not False
        or not isinstance(events, list)
        or not events
        or counts != {"bm25": len(events), "semantic": 0, "encode": 0}
    ):
        raise ValueError("Semble native capture did not execute lexical-only")
    if any(event.get("lane_entry_counts") != {"bm25": 1, "semantic": 0} for event in events):
        raise ValueError("Semble event entered a non-lexical lane")
    if (
        report.get("query_pack_sha256") != digest(canonical(pack))
        or report.get("repository_commit") != suite.get("repository_commit")
        or report.get("file_universe_digest") != suite.get("file_universe_digest")
    ):
        raise ValueError("pair report does not bind the lexical pack and corpus")
    if report.get("sample_count") != task_count:
        raise ValueError("pair report task count differs")
    routes = report.get("rank_metrics", {}).get("routes", {})
    result = {}
    for route, label in (("lexical", "quanta_lexical"), ("semble-hybrid", "semble_lexical_only")):
        data = routes.get(route)
        if not isinstance(data, dict) or data.get("sample_count") != task_count:
            raise ValueError(f"pair report {route} is incomplete")
        recall = data.get("chunk", {}).get("file_recall_at_10")
        if type(recall) not in (int, float) or not 0 <= recall <= 1:
            raise ValueError(f"pair report {route} file recall is invalid")
        hits = round(recall * task_count)
        if abs(hits / task_count - recall) > 1e-10:
            raise ValueError(
                f"pair report {route} file recall does not have task-count granularity"
            )
        result[label] = {"hits": hits, "tasks": task_count, "file_recall_at_10": recall}
    return {
        "routes": result,
        "report_sha256": _sha(path),
        "protocol_lock_sha256": _sha(lock_path),
        "semble_native_sha256": _sha(native_path),
        "verdict_sha256": _sha(verdict_path),
        "semble_lane_calls": counts,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", type=Path, required=True)
    parser.add_argument("--query-pack", type=Path, required=True)
    parser.add_argument("--pair-report", type=Path, required=True)
    parser.add_argument("--pair-lock", type=Path, required=True)
    parser.add_argument("--semble-native", type=Path, required=True)
    parser.add_argument("--pair-verdict", type=Path, required=True)
    for name in PRODUCTS:
        parser.add_argument(f"--{name}-rows", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    suite, pack = _read(args.suite), _read(args.query_pack)
    expected = _tasks(suite, pack)
    result = {
        "status": "diagnostic_unqualified",
        "query_form": "bare_symbol_v1",
        "metric": "file_recall_at_10",
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "suite_sha256": _sha(args.suite),
        "query_pack_sha256": _sha(args.query_pack),
        "validator_sha256": _sha(Path(__file__)),
        "pair": pair_result(
            args.pair_report,
            args.pair_lock,
            args.semble_native,
            args.pair_verdict,
            pack,
            suite,
            len(expected),
        ),
        "products": {
            name: product_result(name, getattr(args, f"{name}_rows"), expected) for name in PRODUCTS
        },
        "exclusions": [
            "independent_gold",
            "native_rank_equivalence",
            "qualified_latency",
            "backend_indexed_universe_attestation",
        ],
    }
    if args.out.exists():
        raise ValueError("output already exists")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
