#!/usr/bin/env python3
"""Rescore one retained five-product lexical capture without merging executions."""

from __future__ import annotations

import argparse
import json
import math
import zipfile
from pathlib import Path

from tools.benchmark.retrieval import lexical_external_oracle as external
from tools.benchmark.retrieval import lexical_file_comparison as comparison
from tools.benchmark.retrieval import source_oracle_suite
from tools.benchmark.retrieval.evaluator import canonical, digest

NATIVE_FILES = {
    "quanta": (
        "rep-00/quanta/quanta-pack.json",
        "rep-00/quanta/strategy-00-fixed_window_strict/record.json",
        "lexical",
    ),
    "semble": (
        "semble-pack.json",
        "rep-00/semble/record.json",
        "semble-lexical-only",
    ),
}
PAIR_FILES = (
    "report-semble-lexical-only-vs-lexical-fixed_window_strict.json",
    "protocol-lock.json",
    "rep-00/semble/native.json",
    "verdict.json",
)
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
MAX_CONTROL_ENTRY_BYTES = 16 * 1024 * 1024


def score_record(
    record: dict,
    native_pack: dict,
    original_pack: dict,
    original_gold: dict[str, set[str]],
    scoring_gold: dict[str, set[str]],
    universe: dict[str, str],
    route: str,
    report_rows: dict[str, dict],
) -> dict:
    tasks = original_pack["tasks"]
    native_tasks = native_pack.get("tasks")
    if (
        record.get("query_pack_sha256") != digest(canonical(native_pack))
        or not isinstance(native_tasks, list)
        or any(not isinstance(task, dict) for task in native_tasks)
        or [
            (task.get("task_id"), task.get("query"), task.get("query_sha256"))
            for task in native_tasks
        ]
        != [(task["task_id"], task["query"], task["query_sha256"]) for task in tasks]
        or not isinstance(record.get("results"), list)
        or len(record["results"]) != len(tasks)
    ):
        raise ValueError(f"{route}: native record and frozen queries differ")
    by_id = {task["task_id"]: task for task in tasks}
    rows = []
    seen = set()
    for result in record["results"]:
        if not isinstance(result, dict):
            raise ValueError(f"{route}: malformed native result")
        task_id = result.get("task_id")
        if task_id not in by_id or task_id in seen or result.get("route") != route:
            raise ValueError(f"{route}: duplicate, unknown, or wrong-route result")
        seen.add(task_id)
        query = by_id[task_id]
        identity = result.get("query_identity")
        candidates = result.get("candidates")
        status = result.get("status")
        if (
            not isinstance(identity, dict)
            or identity.get("original_query_sha256") != query["query_sha256"]
            or status not in {"success", "capped", "abstained"}
            or result.get("error") is not None
            or not isinstance(candidates, list)
            or len(candidates) > 10
            or (status == "abstained") != (len(candidates) == 0)
        ):
            raise ValueError(f"{route}: {task_id} invalid execution or query identity")
        chunk_paths = []
        for rank, candidate in enumerate(candidates, 1):
            if not isinstance(candidate, dict):
                raise ValueError(f"{route}: {task_id} malformed candidate")
            path = candidate.get("path")
            if (
                type(candidate.get("rank")) is not int
                or candidate["rank"] != rank
                or not comparison._canonical_result_path(path)
                or universe.get(path) != candidate.get("file_sha256")
            ):
                raise ValueError(f"{route}: {task_id} candidate rank, path, or hash differs")
            chunk_paths.append(path)
        report = report_rows.get(task_id)
        capture_hit = (
            bool(set(chunk_paths) & original_gold[task_id])
            if original_gold[task_id]
            else "not_applicable"
        )
        if (
            not isinstance(report, dict)
            or report.get("status") != status
            or report.get("candidates") != len(candidates)
            or report.get("file_hit_at_10") != capture_hit
        ):
            raise ValueError(f"{route}: {task_id} native record differs from scored report")
        distinct_paths = list(dict.fromkeys(chunk_paths))
        gold = scoring_gold[task_id]
        matched = gold.intersection(distinct_paths)
        ideal = sum(1 / math.log2(rank + 1) for rank in range(1, min(len(gold), 10) + 1))
        observed = sum(
            1 / math.log2(rank + 1) for rank, path in enumerate(distinct_paths, 1) if path in gold
        )
        rows.append(
            {
                "task_id": task_id,
                "status": status,
                "chunk_count": len(chunk_paths),
                "distinct_file_count": len(distinct_paths),
                "paths_in_chunk_prefix": chunk_paths,
                "file_hit_in_chunk_prefix": bool(matched) if gold else "not_applicable",
                "file_recall_in_chunk_prefix": len(matched) / len(gold)
                if gold
                else "not_applicable",
                "file_ndcg_in_chunk_prefix": observed / ideal if gold else "not_applicable",
            }
        )
    if seen != set(by_id):
        raise ValueError(f"{route}: missing native results")
    positive = [row for row in rows if row["file_hit_in_chunk_prefix"] != "not_applicable"]
    return {
        "tasks": len(tasks),
        "answerable_tasks": len(positive),
        "rank_unit": "chunk",
        "file_hit_in_chunk_prefix": sum(row["file_hit_in_chunk_prefix"] for row in positive),
        "file_recall_in_chunk_prefix": (
            math.fsum(row["file_recall_in_chunk_prefix"] for row in positive) / len(positive)
            if positive
            else "not_applicable"
        ),
        "observed_prefix_file_ndcg": (
            math.fsum(row["file_ndcg_in_chunk_prefix"] for row in positive) / len(positive)
            if positive
            else "not_applicable"
        ),
        "mean_distinct_files_in_10_chunks": (
            math.fsum(row["distinct_file_count"] for row in rows) / len(rows)
        ),
        "ten_distinct_files_in_10_chunks": sum(row["distinct_file_count"] == 10 for row in rows),
        "capped_tasks": sum(row["status"] == "capped" for row in rows),
        "per_query": sorted(rows, key=lambda row: row["task_id"]),
    }


def _evidence_archive(
    evidence_path: Path, archive_path: Path, suite_raw: bytes, pack_raw: bytes
) -> dict:
    evidence = comparison._read(evidence_path)
    source = evidence.get("source")
    command = evidence.get("command")
    verdict = evidence.get("verdict")
    inputs = evidence.get("inputs")
    raw = evidence.get("raw")
    if (
        evidence.get("family") != "retrieval-pair"
        or evidence.get("case_id") != "fixed_window_strict.lexical.context"
        or not isinstance(source, dict)
        or source.get("dirty") is not False
        or not isinstance(source.get("revision"), str)
        or not isinstance(command, dict)
        or command.get("status") != "completed"
        or command.get("exit_code") != 0
        or verdict != {"metrics": [], "reason": None, "scope": "diagnostic", "status": "pass"}
        or not isinstance(inputs, list)
        or not isinstance(raw, list)
    ):
        raise ValueError("native archive evidence is not a completed clean lexical pair")
    input_digests = {
        item.get("id"): item.get("digest") for item in inputs if isinstance(item, dict)
    }
    if input_digests.get("suite") != "sha256:" + digest(suite_raw) or input_digests.get(
        "query_pack"
    ) != "sha256:" + digest(pack_raw):
        raise ValueError("native archive evidence does not bind original suite and pack")
    archive_rows = [
        item for item in raw if isinstance(item, dict) and item.get("path") == "raw/native-tree.zip"
    ]
    if len(archive_rows) != 1 or not archive_path.is_file():
        raise ValueError("native archive is absent from its evidence")
    row = archive_rows[0]
    if (
        archive_path.stat().st_size > MAX_ARCHIVE_BYTES
        or row.get("bytes") != archive_path.stat().st_size
        or row.get("sha256") != "sha256:" + comparison._sha(archive_path)
    ):
        raise ValueError("native archive digest or byte count differs from evidence")
    return {
        "source_revision": source["revision"],
        "evidence_sha256": comparison._sha(evidence_path),
        "archive_sha256": comparison._sha(archive_path),
    }


def evaluate(paths: dict[str, Path], repo: Path, evidence_path: Path, archive_path: Path) -> dict:
    result = external.evaluate(paths, repo)
    suite_raw, pack_raw = comparison._bytes(paths["suite"]), comparison._bytes(paths["query_pack"])
    suite, pack = comparison._json(suite_raw), comparison._json(pack_raw)
    proof = _evidence_archive(evidence_path, archive_path, suite_raw, pack_raw)
    universe = {item["path"]: item["file_sha256"] for item in suite["file_universe"]}
    original_gold = {
        task["task_id"]: {label["path"] for label in task["gold"]} for task in suite["tasks"]
    }
    derived = source_oracle_suite.derive_suites(repo, suite)
    with zipfile.ZipFile(archive_path) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)):
            raise ValueError("native archive has duplicate entries")

        def read(name: str) -> bytes:
            info = archive.getinfo(name)
            if info.file_size > MAX_CONTROL_ENTRY_BYTES:
                raise ValueError(f"native control entry exceeds limit: {name}")
            return archive.read(info)

        if read("evaluator-only/suite.json") != suite_raw or read("query-pack.json") != pack_raw:
            raise ValueError("native archive suite or pack differs from external capture")
        pair_raw = [read(name) for name in PAIR_FILES]
        pair = comparison.pair_result_raw(pair_raw, pack, suite, len(pack["tasks"]))
        pair_report = comparison._json(pair_raw[0])
        for mode in external.MODES:
            oracle_suite = derived[mode][0]
            gold = {
                task["task_id"]: {item["path"] for item in task["file_judgments"]}
                for task in oracle_suite["tasks"]
            }
            native = {}
            for product, (pack_name, record_name, route) in NATIVE_FILES.items():
                native_pack = comparison._json(read(pack_name))
                record_raw = read(record_name)
                record = comparison._json(record_raw)
                report_rows = {
                    row["task_id"]: row
                    for row in pair_report["per_query"]
                    if row.get("route") == route
                }
                scored = score_record(
                    record, native_pack, pack, original_gold, gold, universe, route, report_rows
                )
                scored["record_sha256"] = digest(record_raw)
                native[product] = scored
            result["modes"][mode]["native_products"] = native
    result["native_pair"] = {**proof, "pair_report_sha256": pair["report_sha256"]}
    result["rank_unit_equivalence"] = "non_equivalent"
    result["capture_relation"] = (
        "same_original_suite_pack_and_corpus_separate_native_and_external_capture_processes"
    )
    result["validator_sources_sha256"]["lexical_five_product_oracle.py"] = comparison._sha(
        Path(__file__)
    )
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--native-evidence", required=True, type=Path)
    parser.add_argument("--native-archive", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    for role in external.ROLES:
        parser.add_argument("--" + role.replace("_", "-"), required=True, type=Path)
    args = parser.parse_args()
    try:
        if not args.out.is_absolute() or args.out.exists() or args.out.is_symlink():
            raise ValueError("output must be a new absolute path")
        paths = {role: getattr(args, role) for role in external.ROLES}
        result = evaluate(paths, args.repo, args.native_evidence, args.native_archive)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("x", encoding="utf-8") as stream:
            json.dump(result, stream, indent=2, sort_keys=True)
            stream.write("\n")
    except (OSError, ValueError, zipfile.BadZipFile, KeyError) as exc:
        parser.error(str(exc))


if __name__ == "__main__":
    main()
