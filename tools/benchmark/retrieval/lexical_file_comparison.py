#!/usr/bin/env python3
"""Validate a frozen lexical diagnostic across code-search products.

Latency is descriptive only: endpoints have different timing layers, so this
tool does not calculate a cross-product speed ratio or ranking. Mechanical
labels are not an independent quality oracle.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import statistics
import subprocess
import tempfile
from pathlib import Path

from tools.benchmark.evidence import (
    CONTROL_DOCUMENT_BYTES,
    RawFile,
    _read_control_file,
    file_digest,
)
from tools.benchmark.retrieval.evaluator import (
    canonical,
    declared_evaluation_contract,
    digest,
    validate_comparison_contract,
)
from tools.benchmark.retrieval.finite_json import is_finite_json_number
from tools.benchmark.retrieval.query_plan import (
    CODE_SEARCH_FILE_POLICIES,
    QUANTA_EVALUATION_POLICIES,
    execution_profile,
)

PRODUCTS = ("sourcegraph", "opengrok", "cs")
QUANTA_LEXICAL_ROUTE = "lexical"
SEMBLE_LEXICAL_ROUTE = "semble-lexical-only"
BARE_SYMBOL = re.compile(r"[A-Za-z_][A-Za-z_0-9]*\Z")
INPUT_ROLES = (
    "suite",
    "query_pack",
    "pair_report",
    "pair_lock",
    "semble_native",
    "pair_verdict",
    "sourcegraph_rows",
    "opengrok_rows",
    "cs_rows",
)
FILE_INPUT_ROLES = (
    *INPUT_ROLES,
    "quanta_record",
    "semble_record",
    "source_bundle",
    "pair_manifest",
    "quanta_phase_metrics",
    "semble_phase_metrics",
)
FILE_ROUTES = [QUANTA_LEXICAL_ROUTE, "semble-lexical-file"]
MAX_NATIVE_TRACE_BYTES = 128 * 1024 * 1024
TIMING_LAYERS = {
    "sourcegraph": "loopback_stream_http_request_wall",
    "opengrok": "loopback_rest_http_request_wall",
    "cs": "process_spawn_and_search_wall",
    "quanta_lexical": "runner_sdk_query_call",
    "semble_lexical_only": "worker_search_dispatch_call",
    "semble_lexical_file": "worker_search_dispatch_call",
}


def input_roles(value: dict) -> tuple[str, ...]:
    """Closed input inventories; current file results require retained raw records."""
    inputs = value.get("inputs", value)
    keys = set(inputs) - {"schema_version"}
    if keys == set(FILE_INPUT_ROLES):
        return FILE_INPUT_ROLES
    if keys == set(INPUT_ROLES):
        return INPUT_ROLES
    raise ValueError("lexical input role inventory differs; current file pair requires raw roles")


def _file_policy_from_lock(lock: dict) -> str:
    profiles = lock.get("execution_profiles")
    quanta = profiles.get("quanta") if isinstance(profiles, dict) else None
    policy = quanta.get("policy") if isinstance(quanta, dict) else None
    if policy not in CODE_SEARCH_FILE_POLICIES:
        raise ValueError("current file pair requires a code-search file policy")
    return policy


def sourcegraph_capability(query: str) -> dict:
    from tools.benchmark.retrieval import sourcegraph

    if sourcegraph.SAFE_QUERY.fullmatch(query) is None:
        reason = "sourcegraph_conservative_keyword_shape"
    elif any(
        term.casefold() in sourcegraph.BOOLEAN_OPERATORS or term.startswith("-")
        for term in query.split()
    ):
        reason = "sourcegraph_reserved_keyword_token"
    else:
        return {"status": "supported", "reason": None}
    return {"status": "unsupported", "reason": reason}


def latency_summary(values: list[object], expected_count: int, layer: str) -> dict:
    if len(values) != expected_count or any(
        not is_finite_json_number(value) or value < 0 for value in values
    ):
        raise ValueError(f"{layer}: missing, non-finite or invalid latency observation")
    ordered = sorted(values)
    return {
        "count": len(ordered),
        "timing_layer": layer,
        "mean_ms": statistics.mean(ordered),
        "p50_ms": statistics.median(ordered),
        "p95_ms": ordered[math.ceil(0.95 * len(ordered)) - 1],
        "min_ms": ordered[0],
        "max_ms": ordered[-1],
        "p95_definition": "nearest_rank",
    }


def _bytes(path: Path) -> bytes:
    """Lexical control JSON is bounded; observation JSONL uses the line owner."""
    return _read_control_file(path)


def _unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def _json(data: bytes) -> dict:
    def invalid_constant(value):
        raise ValueError(f"non-finite JSON constant: {value}")

    value = json.loads(
        data.decode("utf-8"), object_pairs_hook=_unique_object, parse_constant=invalid_constant
    )
    if not isinstance(value, dict):
        raise ValueError("lexical input must be a JSON object")
    return value


def _read(path: Path) -> dict:
    return _json(_bytes(path))


def _sha(path: Path) -> str:
    return file_digest(path)[0].removeprefix("sha256:")


def _file_universe(suite: dict, pack: dict) -> set[str]:
    files = suite.get("file_universe")
    if not isinstance(files, list) or not files or pack.get("file_universe") != files:
        raise ValueError("suite and pack need the same nonempty file universe")
    if (
        suite.get("file_universe_digest") != digest(canonical(files))
        or pack.get("file_universe_digest") != suite["file_universe_digest"]
    ):
        raise ValueError("suite/pack file universe digest differs")
    paths: set[str] = set()
    for file in files:
        if not isinstance(file, dict) or set(file) != {"path", "file_sha256"}:
            raise ValueError("malformed file universe entry")
        path, sha = file["path"], file["file_sha256"]
        if (
            not _canonical_result_path(path)
            or not isinstance(sha, str)
            or re.fullmatch(r"[0-9a-f]{64}", sha) is None
            or path in paths
        ):
            raise ValueError("noncanonical or duplicate file universe entry")
        paths.add(path)
    if [file["path"] for file in files] != sorted(paths):
        raise ValueError("file universe must be sorted by unique path")
    return paths


def _canonical_result_path(path: object) -> bool:
    return (
        isinstance(path, str)
        and bool(path)
        and not path.startswith("/")
        and "\\" not in path
        and "\x00" not in path
        and all(
            part not in ("", ".", "..") and part.casefold() != ".git" for part in path.split("/")
        )
    )


def _tasks(
    suite: dict,
    pack: dict,
    *,
    file_policy: str = "code_search_file",
    allow_single_lexical: bool = False,
) -> dict[str, tuple[str, list[str]]]:
    if pack.get("suite_commitment_sha256") != digest(canonical(suite)):
        raise ValueError("pack and suite commitment differ")
    if (
        type(suite.get("schema_version")) is not int
        or suite["schema_version"] != 3
        or type(pack.get("schema_version")) is not int
        or pack["schema_version"] != 3
        or not isinstance(suite.get("suite_id"), str)
        or not suite["suite_id"].strip()
        or pack.get("suite_id") != suite["suite_id"]
    ):
        raise ValueError("suite and pack metadata differ from the v3 lexical contract")
    if suite.get("repository_commit") != pack.get("repository_commit"):
        raise ValueError("pack and suite repository commits differ")
    if pack.get("comparison_contract") != suite.get("comparison_contract"):
        raise ValueError("pack and suite comparison contract differ")
    contract = validate_comparison_contract(suite.get("comparison_contract"), "lexical contract")
    if contract["top_k"] != 10:
        raise ValueError("lexical diagnostic requires top_k 10 in suite and pack")
    routes = suite.get("routes")
    single_file_route = allow_single_lexical and routes == [QUANTA_LEXICAL_ROUTE]
    if (
        routes not in ([QUANTA_LEXICAL_ROUTE, SEMBLE_LEXICAL_ROUTE], FILE_ROUTES)
        and not single_file_route
    ) or pack.get("routes") != routes:
        raise ValueError("suite and pack route inventory is not the lexical pair")
    if (
        pack.get("tokenizer") != contract["tokenizer"]
        or pack.get("tokenizer_budget_version") != contract["tokenizer_budget_version"]
    ):
        raise ValueError("pack metadata tokenizer contract differs")
    pack_tasks = pack.get("tasks")
    suite_tasks = suite.get("tasks")
    if (
        not isinstance(pack_tasks, list)
        or not isinstance(suite_tasks, list)
        or len(pack_tasks) != len(suite_tasks)
        or not pack_tasks
    ):
        raise ValueError("pack and suite task counts differ or are empty")
    evaluation = declared_evaluation_contract(suite_tasks)
    if (
        evaluation is not None
        and file_policy not in QUANTA_EVALUATION_POLICIES[evaluation["request_mode"]]
    ):
        raise ValueError("native file policy differs from the evaluation request mode")
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
            or (
                routes != FILE_ROUTES
                and not single_file_route
                and BARE_SYMBOL.fullmatch(query) is None
            )
            or task_id in blinded
        ):
            raise ValueError("duplicate or malformed blinded query")
        if routes == FILE_ROUTES or single_file_route:
            from tools.benchmark.retrieval.query_plan import derive_query_identity

            derive_query_identity(file_policy, query)
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
        if (
            not isinstance(gold, list)
            or task.get("answerable") is not bool(gold)
            or any(
                not isinstance(label, dict)
                or not isinstance(label.get("path"), str)
                or not label["path"]
                for label in gold
            )
        ):
            raise ValueError("lexical task requires judged answerability and file labels")
        paths = sorted({label["path"] for label in gold})
        expected[task_id] = task["query"], paths
    return expected


def product_result(
    product: str,
    path: Path,
    expected: dict[str, tuple[str, list[str]]],
    universe: set[str],
    *,
    scoring_gold: dict[str, list[str]] | None = None,
    file_judgments: dict[str, list[dict]] | None = None,
) -> dict:
    if file_judgments is not None:
        if set(file_judgments) != set(expected):
            raise ValueError(f"{product}: independent file judgment inventory differs")
        scoring_gold = {
            task: [row["path"] for row in rows if row["grade"] > 0]
            for task, rows in file_judgments.items()
        }
    if product not in PRODUCTS:
        raise ValueError(f"unknown lexical product: {product}")

    if not expected or any(
        not isinstance(gold, list)
        or any(not isinstance(value, str) or not value for value in gold)
        or len(gold) != len(set(gold))
        for _, gold in expected.values()
    ):
        raise ValueError(f"{product}: invalid or duplicate golden file inventory")
    if scoring_gold is not None and (
        set(scoring_gold) != set(expected)
        or any(
            not isinstance(gold, list)
            or any(not _canonical_result_path(value) or value not in universe for value in gold)
            or len(gold) != len(set(gold))
            for gold in scoring_gold.values()
        )
    ):
        raise ValueError(f"{product}: invalid scoring gold inventory")
    raw = RawFile.capture(path)

    def consume(lines):
        seen: set[str] = set()
        hits = empty_no_gold = metadata_bytes = 0
        unsupported = []
        elapsed: list[object] = []
        per_query = []
        for line in lines:
            row = _json(line)
            if row.get("lane") != "symbol_only":
                continue  # Other recorded lanes are not scored by this diagnostic.
            task_id = row.get("task_id")
            if not isinstance(task_id, str) or task_id not in expected or task_id in seen:
                raise ValueError(f"{product}: missing or duplicate task")
            seen.add(task_id)
            query, gold = expected[task_id]
            if row.get("submitted_query") != query or row.get("gold_paths") != gold:
                raise ValueError(f"{product}: {task_id} query or gold differs")
            if row.get("status") == "unsupported":
                capability = (
                    sourcegraph_capability(query)
                    if product == "sourcegraph"
                    else {"status": "supported", "reason": None}
                )
                if capability["status"] != "unsupported" or row != {
                    "lane": "symbol_only",
                    "task_id": task_id,
                    "submitted_query": query,
                    "gold_paths": gold,
                    "status": "unsupported",
                    "capability_reason": capability["reason"],
                }:
                    raise ValueError(f"{product}: {task_id} forged unsupported capability")
                unsupported.append(task_id)
                per_query.append(
                    {
                        "task_id": task_id,
                        "status": "unsupported",
                        "capability_reason": capability["reason"],
                        "file_hit_at_10": "not_applicable",
                        "file_recall_at_10": "not_applicable",
                        "no_gold_empty_at_10": "not_applicable",
                        "query_latency_ms": None,
                    }
                )
                continue
            if row.get("status", "success") != "success" or (
                product == "sourcegraph" and sourcegraph_capability(query)["status"] != "supported"
            ):
                raise ValueError(
                    f"{product}: {task_id} malformed status or unsupported query execution"
                )
            if product == "cs":
                if type(row.get("exit_code")) is not int or row["exit_code"] != 0:
                    raise ValueError(f"{product}: {task_id} failed process")
                paths = row.get("paths")
            else:
                if (
                    type(row.get("http_status")) is not int
                    or row["http_status"] != 200
                    or row.get("error") is not None
                ):
                    raise ValueError(f"{product}: {task_id} failed request")
                if product == "opengrok" and row.get("field") != "full":
                    raise ValueError(f"{product}: {task_id} used a non-full field")
                paths = row.get("file_paths_top_10")
            if (
                not isinstance(paths, list)
                or len(paths) > 10
                or any(
                    not _canonical_result_path(value) or value not in universe for value in paths
                )
                or len(paths) != len(set(paths))
            ):
                raise ValueError(f"{product}: {task_id} malformed result paths")
            if row.get("file_hit_at_10") is not bool(set(paths) & set(gold)):
                raise ValueError(f"{product}: {task_id} hit flag differs from paths")
            judged_gold = scoring_gold[task_id] if scoring_gold is not None else gold
            hit = bool(set(paths) & set(judged_gold))
            hits += hit
            if not judged_gold:
                empty_no_gold += not paths
            elapsed.append(row.get("elapsed_ms"))
            per_query.append(
                {
                    "task_id": task_id,
                    "file_hit_at_10": hit if judged_gold else "not_applicable",
                    "file_recall_at_10": (
                        len(set(paths) & set(judged_gold)) / len(judged_gold)
                        if judged_gold
                        else "not_applicable"
                    ),
                    "no_gold_empty_at_10": not paths if not judged_gold else "not_applicable",
                    "query_latency_ms": row.get("elapsed_ms"),
                }
            )
            if scoring_gold is not None:
                ideal = sum(
                    1 / math.log2(rank + 1) for rank in range(1, min(len(judged_gold), 10) + 1)
                )
                observed = sum(
                    1 / math.log2(rank + 1)
                    for rank, value in enumerate(paths, 1)
                    if value in judged_gold
                )
                per_query[-1]["file_ndcg_at_10"] = (
                    observed / ideal if judged_gold else "not_applicable"
                )
            if file_judgments is not None and judged_gold:
                from tools.benchmark.retrieval.evaluator import file_ndcg_at_k

                per_query[-1]["file_ndcg_at_10"] = file_ndcg_at_k(
                    [{"path": value} for value in paths], file_judgments[task_id], 10
                )
            metadata_bytes += len(canonical(per_query[-1]))
            if metadata_bytes > CONTROL_DOCUMENT_BYTES:
                raise ValueError("lexical result metadata exceeds explicit control byte limit")
        if len(seen) != len(expected):
            raise ValueError(f"{product}: incomplete symbol-only lane")
        return hits, empty_no_gold, elapsed, per_query, unsupported

    hits, empty_no_gold, elapsed, per_query, unsupported = raw.consume_lines(consume)
    answerable = sum(
        bool(scoring_gold[task_id] if scoring_gold is not None else gold)
        for task_id, (_, gold) in expected.items()
    )
    no_gold = len(expected) - answerable
    supported_answerable = answerable - sum(
        bool(scoring_gold[task_id] if scoring_gold is not None else expected[task_id][1])
        for task_id in unsupported
    )
    supported_no_gold = no_gold - (len(unsupported) - (answerable - supported_answerable))
    result = {
        "hits": hits,
        "tasks": len(expected),
        "rank_unit": "distinct_file",
        "answerable_tasks": answerable,
        "no_gold_tasks": no_gold,
        "file_hit_rate_at_10": hits / supported_answerable
        if supported_answerable
        else "not_applicable",
        "file_recall_at_10": (
            math.fsum(
                row["file_recall_at_10"]
                for row in per_query
                if row["file_recall_at_10"] != "not_applicable"
            )
            / supported_answerable
            if supported_answerable
            else "not_applicable"
        ),
        "no_gold_empty_rate_at_10": empty_no_gold / supported_no_gold
        if supported_no_gold
        else "not_applicable",
        "per_query": sorted(per_query, key=lambda row: row["task_id"]),
        "latency_ms": latency_summary(elapsed, len(elapsed), TIMING_LAYERS[product])
        if elapsed
        else {"count": 0, "timing_layer": TIMING_LAYERS[product], "status": "not_run"},
        "raw_sha256": raw.sha256.removeprefix("sha256:"),
    }
    if unsupported:
        result["capability_coverage"] = {
            "requested": len(expected),
            "supported": len(expected) - len(unsupported),
            "unsupported": len(unsupported),
            "unsupported_task_ids": sorted(unsupported),
            "supported_answerable_tasks": supported_answerable,
            "supported_no_gold_tasks": supported_no_gold,
            "metric_denominator": "supported_judged_tasks_only",
        }
    if scoring_gold is not None:
        result["file_ndcg_at_10"] = (
            math.fsum(
                row["file_ndcg_at_10"]
                for row in per_query
                if row.get("file_ndcg_at_10", "not_applicable") != "not_applicable"
            )
            / supported_answerable
            if supported_answerable
            else "not_applicable"
        )
    return result


def pair_result(
    path: Path,
    lock_path: Path,
    native_path: Path,
    verdict_path: Path,
    pack: dict,
    suite: dict,
    task_count: int,
) -> dict:
    raw = [_bytes(value) for value in (path, lock_path, native_path, verdict_path)]
    return pair_result_raw(raw, pack, suite, task_count)


def pair_result_raw(raw: list[bytes], pack: dict, suite: dict, task_count: int) -> dict:
    """Validate a pair capture already bound to retained archive bytes."""
    if len(raw) != 4 or any(not isinstance(value, bytes) for value in raw):
        raise ValueError("pair capture requires report, lock, native trace, and verdict bytes")
    report, lock, native, verdict = [_json(value) for value in raw]
    states = verdict.get("states")
    if not isinstance(states, dict) or states.get("PAIR_VALID") != "pass":
        raise ValueError("pair verdict is not valid")
    profiles = lock.get("execution_profiles", {})
    if (
        not isinstance(profiles, dict)
        or profiles.get("quanta") != execution_profile("native")
        or profiles.get("semble")
        != {
            "profile_id": "semble-lexical-only-v1",
            "mode": "lexical-only",
            "alpha": None,
            "rerank": "not_applicable",
        }
    ):
        raise ValueError("pair execution profiles are not pure lexical")
    if (
        lock.get("quanta_routes") != [QUANTA_LEXICAL_ROUTE]
        or lock.get("semble_route") != SEMBLE_LEXICAL_ROUTE
    ):
        raise ValueError("pair route labels do not match the pure-lexical execution profiles")
    if (
        type(lock.get("top_k")) is not int
        or lock["top_k"] != 10
        or report.get("comparison_contract") != suite.get("comparison_contract")
    ):
        raise ValueError("pair top_k contract differs from the lexical suite")
    comparisons = verdict.get("comparisons")
    record_digest = report.get("runner_record_sha256")
    if (
        not isinstance(comparisons, list)
        or len(comparisons) != 1
        or not isinstance(comparisons[0], dict)
        or not isinstance(comparisons[0].get("strategy"), str)
        or not comparisons[0]["strategy"]
        or not isinstance(record_digest, str)
        or re.fullmatch(r"[0-9a-f]{64}", record_digest) is None
        or lock.get("strategies") != [comparisons[0].get("strategy")]
        or comparisons[0].get("candidate_route") != QUANTA_LEXICAL_ROUTE
        or comparisons[0].get("baseline_route") != SEMBLE_LEXICAL_ROUTE
        or comparisons[0].get("report_digest") != hashlib.sha256(raw[0]).hexdigest()
        or comparisons[0].get("record_digest") != record_digest
    ):
        raise ValueError("pair verdict does not bind the lexical report and runner record")
    observed_counts = verdict.get("counts")
    if observed_counts != {
        "selected": 2 * task_count,
        "executed": 2 * task_count,
        "passed": 2 * task_count,
        "failed": 0,
    } or any(type(value) is not int for value in observed_counts.values()):
        raise ValueError("pair verdict execution counts differ from the lexical task inventory")
    counts = native.get("lane_call_counts", {})
    events = native.get("execution_events")
    if (
        native.get("semble_profile") != "lexical-only"
        or native.get("rerank_applied") is not False
        or not isinstance(events, list)
        or not events
        or counts != {"bm25": len(events), "semantic": 0, "encode": 0}
        or any(type(value) is not int for value in counts.values())
    ):
        raise ValueError("Semble native capture did not execute lexical-only")
    if any(
        not isinstance(event, dict)
        or event.get("lane_entry_counts") != {"bm25": 1, "semantic": 0}
        or any(type(value) is not int for value in event["lane_entry_counts"].values())
        for event in events
    ):
        raise ValueError("Semble event entered a non-lexical lane")
    measured_tasks = [event.get("task_id") for event in events if event.get("phase") == "measured"]
    if (
        any(not isinstance(task_id, str) for task_id in measured_tasks)
        or len(measured_tasks) != task_count
        or set(measured_tasks) != {task["task_id"] for task in pack["tasks"]}
    ):
        raise ValueError("Semble native measured task coverage differs from the lexical pack")
    if (
        report.get("query_pack_sha256") != digest(canonical(pack))
        or report.get("repository_commit") != suite.get("repository_commit")
        or report.get("file_universe_digest") != suite.get("file_universe_digest")
    ):
        raise ValueError("pair report does not bind the lexical pack and corpus")
    if report.get("sample_count") != task_count:
        raise ValueError("pair report task count differs")
    metrics = report.get("rank_metrics")
    if not isinstance(metrics, dict) or not isinstance(metrics.get("routes"), dict):
        raise ValueError("pair report has malformed rank metric routes")
    routes = metrics["routes"]
    per_query = report.get("per_query")
    if (
        not isinstance(per_query, list)
        or len(per_query) != 2 * task_count
        or any(
            not isinstance(row, dict)
            or not isinstance(row.get("task_id"), str)
            or not isinstance(row.get("route"), str)
            for row in per_query
        )
    ):
        raise ValueError("pair report has incomplete per-query observations")
    judged = {
        task["task_id"]: bool(task["gold"])
        for task in suite["tasks"]
        if isinstance(task, dict)
        and isinstance(task.get("task_id"), str)
        and isinstance(task.get("gold"), list)
    }
    if len(judged) != task_count or set(judged) != {task["task_id"] for task in pack["tasks"]}:
        raise ValueError("pair report suite task inventory differs")
    answerable = sum(judged.values())
    no_gold = task_count - answerable
    result = {}
    for route, label in (
        (QUANTA_LEXICAL_ROUTE, "quanta_lexical"),
        (SEMBLE_LEXICAL_ROUTE, "semble_lexical_only"),
    ):
        data = routes.get(route)
        if not isinstance(data, dict) or data.get("sample_count") != task_count:
            raise ValueError(f"pair report {route} is incomplete")
        chunk = data.get("chunk")
        if not isinstance(chunk, dict):
            raise ValueError(f"pair report {route} has malformed chunk metrics")
        recall = chunk.get("file_recall_at_10")
        if (answerable and (type(recall) not in (int, float) or not 0 <= recall <= 1)) or (
            not answerable and recall != "not_applicable"
        ):
            raise ValueError(f"pair report {route} file recall is invalid")
        route_rows = [row for row in per_query if row.get("route") == route]
        if len(route_rows) != task_count or {row.get("task_id") for row in route_rows} != {
            task["task_id"] for task in pack["tasks"]
        }:
            raise ValueError(f"pair report {route} per-query tasks differ")
        if any(
            row.get("answerable") is not judged[row["task_id"]]
            or row.get("status") not in {"success", "capped", "abstained"}
            or type(row.get("candidates")) is not int
            or not 0 <= row["candidates"] <= 10
            or (row["status"] == "abstained") != (row["candidates"] == 0)
            for row in route_rows
        ):
            raise ValueError(f"pair report {route} recall/hit observations differ from execution")
        positive_rows = [row for row in route_rows if judged[row["task_id"]]]
        negative_rows = [row for row in route_rows if not judged[row["task_id"]]]
        recalls = [row.get("file_recall_at_10") for row in positive_rows]
        flags = [row.get("file_hit_at_10") for row in positive_rows]
        if (
            any(not is_finite_json_number(value) or not 0 <= value <= 1 for value in recalls)
            or any(type(value) is not bool for value in flags)
            or any(flag != (value > 0) for flag, value in zip(flags, recalls, strict=True))
            or any(
                row["status"] == "abstained"
                and (row["file_recall_at_10"] != 0 or row["file_hit_at_10"] is not False)
                for row in positive_rows
            )
            or (answerable and abs(math.fsum(recalls) / answerable - recall) > 1e-10)
            or any(
                row.get("answerable") is not False
                or row.get("file_recall_at_10") != "not_applicable"
                or row.get("file_hit_at_10") != "not_applicable"
                for row in negative_rows
            )
        ):
            raise ValueError(f"pair report {route} recall/hit observations differ from aggregate")
        hits = sum(flags)
        empty_no_gold = sum(row["candidates"] == 0 for row in negative_rows)
        latency = latency_summary(
            [row.get("query_latency_ms") for row in route_rows],
            task_count,
            TIMING_LAYERS[label],
        )
        reported_mean = data.get("mean_query_latency_ms")
        if (
            not is_finite_json_number(reported_mean)
            or abs(latency["mean_ms"] - reported_mean) > 1e-6
        ):
            raise ValueError(f"pair report {route} mean latency differs from observations")
        result[label] = {
            "hits": hits,
            "tasks": task_count,
            "rank_unit": "chunk",
            "answerable_tasks": answerable,
            "no_gold_tasks": no_gold,
            "file_recall_at_10": recall,
            "file_hit_rate_at_10": hits / answerable if answerable else "not_applicable",
            "no_gold_empty_rate_at_10": (empty_no_gold / no_gold if no_gold else "not_applicable"),
            "per_query": sorted(route_rows, key=lambda row: row["task_id"]),
            "latency_ms": latency,
        }
    return {
        "routes": result,
        **dict(
            zip(
                ("report_sha256", "protocol_lock_sha256", "semble_native_sha256", "verdict_sha256"),
                (hashlib.sha256(value).hexdigest() for value in raw),
                strict=True,
            )
        ),
        "semble_lane_calls": counts,
    }


def file_pair_result(paths: dict[str, Path], suite_raw: bytes, pack_raw: bytes) -> dict:
    """Revalidate the current file pair from its retained Git bundle and raw records."""
    from tools.benchmark.retrieval import evaluator, run

    missing = set(FILE_INPUT_ROLES) - set(paths)
    if missing:
        raise ValueError(f"current file pair is missing raw roles: {sorted(missing)}")
    suite, pack = _json(suite_raw), _json(pack_raw)
    if suite["routes"] != FILE_ROUTES:
        raise ValueError("current file pair requires the frozen file route inventory")
    roles = (
        "pair_report",
        "pair_lock",
        "semble_native",
        "pair_verdict",
        "pair_manifest",
        "quanta_record",
        "semble_record",
        "quanta_phase_metrics",
        "semble_phase_metrics",
    )

    def read_native(stream):
        data = stream.read(MAX_NATIVE_TRACE_BYTES + 1)
        if len(data) > MAX_NATIVE_TRACE_BYTES:
            raise ValueError("native Semble trace exceeds its explicit custody limit")
        return data

    raw = {
        role: RawFile.capture(paths[role]).consume_seekable(read_native)
        if role == "semble_native"
        else _bytes(paths[role])
        for role in roles
    }
    values = {role: _json(data) for role, data in raw.items()}
    lock, verdict, native = (
        values[role] for role in ("pair_lock", "pair_verdict", "semble_native")
    )
    quanta_policy = _file_policy_from_lock(lock)
    expected = _tasks(suite, pack, file_policy=quanta_policy)
    profiles = {
        "quanta": execution_profile(quanta_policy),
        "semble": {
            "profile_id": "semble-lexical-file-v1",
            "mode": "lexical-file",
            "alpha": None,
            "rerank": "not_applicable",
        },
    }
    if (
        lock.get("execution_profiles") != profiles
        or lock.get("execution_profiles_sha256") != digest(canonical(profiles))
        or lock.get("quanta_routes") != ["lexical"]
        or lock.get("semble_route") != "semble-lexical-file"
        or lock.get("top_k") != 10
        or lock.get("suite_digest") != hashlib.sha256(suite_raw).hexdigest()
        or lock.get("query_pack_digest") != hashlib.sha256(pack_raw).hexdigest()
    ):
        raise ValueError(
            "current file pair lock profile, route, or frozen source/query binding differs"
        )
    bundle = RawFile.capture(paths["source_bundle"])
    if not 0 < bundle.size <= 256 * 1024 * 1024:
        raise ValueError("source bundle exceeds its custody limit")
    with tempfile.TemporaryDirectory(prefix="quanta-lexical-file-pair-") as temporary:
        root = Path(temporary).resolve()
        held = root / "source.bundle"
        bundle.copy_to(held)
        repo = root / "repo"
        for argv in (
            ["git", "clone", "--quiet", "--", str(held), str(repo)],
            ["git", "-C", str(repo), "checkout", "--quiet", "--detach", suite["repository_commit"]],
        ):
            completed = subprocess.run(argv, capture_output=True, timeout=60)
            if completed.returncode:
                raise ValueError(
                    "retained source bundle cannot restore the selected corpus revision"
                )
        suite_path = root / "suite.json"
        suite_path.write_bytes(suite_raw)
        record_paths = []
        for role in ("quanta_record", "semble_record"):
            path = root / (role + ".json")
            path.write_bytes(raw[role])
            record_paths.append(path)
        checked_suite, checked_pack, merged = run.merge_records(repo, suite_path, record_paths)
        if checked_suite != suite or checked_pack != pack:
            raise ValueError("raw pair record source, suite, or pack identity differs")
        rebuilt = evaluator.evaluate_paired_file_diagnostic(
            suite, pack, merged, "semble-lexical-file", "lexical"
        )
    if values["pair_report"] != rebuilt:
        raise ValueError("current file pair report differs from raw record replay")
    manifest = values["pair_manifest"]
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, dict):
        raise ValueError("current file pair manifest lacks phase metrics custody")
    phase_refs = artifacts.get("phase_metrics")
    phase_digests = artifacts.get("phase_metrics_digests")
    if (
        not isinstance(phase_refs, list)
        or len(phase_refs) != 2
        or any(not _canonical_result_path(ref) for ref in phase_refs)
        or len(set(phase_refs)) != 2
        or not isinstance(phase_digests, dict)
        or set(phase_digests) != set(phase_refs)
    ):
        raise ValueError("current file pair manifest phase path/digest inventory differs")
    phases = {}
    for system, route in (("quanta", "lexical"), ("semble", "semble-lexical-file")):
        phase_role = system + "_phase_metrics"
        record_role = system + "_record"
        phase_sha = hashlib.sha256(raw[phase_role]).hexdigest()
        matching = [ref for ref in phase_refs if phase_digests[ref] == phase_sha]
        if len(matching) != 1 or system not in Path(matching[0]).parts:
            raise ValueError(f"{system}: phase metrics hash does not bind its manifest role")
        phase = run._validate_phase_metrics(values[phase_role], phase_role)
        if (
            phase["system"] != system
            or phase["record_sha256"] != hashlib.sha256(raw[record_role]).hexdigest()
            or phase["query_schedule"] != list(expected)
            or phase["task_count"] != len(expected)
            or phase["route_count"] != 1
        ):
            raise ValueError(f"{system}: phase metrics source/record/query inventory differs")
        run.validate_completed_query_timing(phase)
        record_rows = {row["task_id"]: row for row in values[record_role]["results"]}
        for entry in phase["query_timing"]["observations"]:
            if entry["route"] != route:
                raise ValueError(f"{system}: completed-response route differs")
            if entry["phase"] == "measured":
                row = record_rows.get(entry["task_id"])
                if row is None or row["status"] != entry["status"]:
                    raise ValueError(f"{system}: completed-response status differs from raw record")
                if entry["iteration"] == 0 and (
                    not is_finite_json_number(row["timings"]["query_latency_ms"])
                    or not math.isclose(
                        row["timings"]["query_latency_ms"],
                        (entry["end_ns"] - entry["start_ns"]) / 1e6,
                        rel_tol=1e-9,
                        abs_tol=1e-9,
                    )
                ):
                    raise ValueError(
                        f"{system}: completed-response latency differs from raw record"
                    )
        phases[system] = phase
    if (
        native.get("query_timing") != phases["semble"]["query_timing"]
        or native.get("query_protocol") != phases["semble"].get("query_protocol")
        or native.get("latencies_ms")
        != phases["semble"].get("warm_latencies_ms", {}).get("semble-lexical-file")
        or native.get("cold_latency_ms")
        != phases["semble"].get("cold_latencies_ms", {}).get("semble-lexical-file")
    ):
        raise ValueError("Semble native completed-response timing differs from raw phase metrics")
    if bundle.sha256 != RawFile.capture(paths["source_bundle"]).sha256:
        raise ValueError("source bundle changed during raw pair replay")
    if set(merged["route_provenance"]) != set(FILE_ROUTES):
        raise ValueError("raw file pair route inventory differs")
    captures = {}
    for route, system in (("lexical", "quanta"), ("semble-lexical-file", "semble")):
        capture = merged["captures"][merged["route_provenance"][route]["capture_id"]]
        if capture["system"] != system or capture["execution_profile"] != profiles[system]:
            raise ValueError("raw file pair execution profile differs from the locked profile")
        captures[system] = capture
    qbinary = captures["quanta"]["runner_binary"]["digest"]
    if (
        phases["quanta"]["runner_binary_sha256"] != qbinary
        or phases["semble"]["worker_sha256"] != captures["semble"]["runner_binary"]["digest"]
        or (
            phases["semble"]["schema_version"] == 2
            and phases["semble"].get("profile") != "lexical-file"
        )
    ):
        raise ValueError("completed-response phase binary/profile differs from the raw capture")
    provenance = values["pair_manifest"].get("provenance")
    if (
        not isinstance(provenance, dict)
        or any(not isinstance(provenance.get(key), dict) for key in ("quanta", "suite", "semble"))
        or not isinstance(verdict.get("provenance"), dict)
        or any(
            not isinstance(values, dict)
            or not isinstance(provenance.get(section), dict)
            or any(provenance[section].get(key) != value for key, value in values.items())
            for section, values in verdict.get("provenance", {}).items()
        )
        or provenance.get("quanta", {}).get("binary_digest") != qbinary
        or re.fullmatch(r"[0-9a-f]{40}", provenance.get("quanta", {}).get("source_sha", "")) is None
        or provenance.get("suite", {}).get("suite_digest") != lock["suite_digest"]
        or provenance.get("suite", {}).get("query_pack_digest") != lock["query_pack_digest"]
        or lock.get("searchd_expected_sha256")
        != captures["quanta"]["searchd_binary"]["binary_digest"]
        or provenance.get("semble", {}).get("lockfile_digest") != lock.get("semble_lockfile_sha256")
    ):
        raise ValueError("file pair source/binary provenance differs from the frozen manifest")
    comparisons = verdict.get("comparisons")
    if (
        not isinstance(comparisons, list)
        or len(comparisons) != 1
        or not isinstance(comparisons[0], dict)
        or comparisons[0].get("candidate_route") != "lexical"
        or comparisons[0].get("baseline_route") != "semble-lexical-file"
        or lock.get("strategies") != [comparisons[0].get("strategy")]
        or comparisons[0].get("report_digest") != hashlib.sha256(raw["pair_report"]).hexdigest()
        or comparisons[0].get("record_digest") != digest(canonical(merged))
    ):
        raise ValueError("file pair verdict does not bind the replayed records and report")
    passed = sum(row["status"] in {"success", "capped"} for row in merged["results"])
    failed = sum(
        row["status"] not in {"success", "capped", "abstained"} for row in merged["results"]
    )
    total = 2 * len(expected)
    if (
        not isinstance(verdict.get("counts"), dict)
        or any(type(value) is not int for value in verdict["counts"].values())
        or verdict.get("counts")
        != {
            "selected": total,
            "executed": total,
            "passed": passed,
            "failed": failed,
        }
        or not isinstance(verdict.get("states"), dict)
        or verdict["states"].get("PAIR_VALID") != ("pass" if failed == 0 else "fail")
    ):
        raise ValueError("file pair verdict terminal counts differ from the raw execution statuses")
    events = native.get("execution_events")
    measured = (
        [event for event in events if event.get("phase") == "measured"]
        if isinstance(events, list) and all(isinstance(event, dict) for event in events)
        else []
    )
    if (
        native.get("semble_profile") != "lexical-file"
        or native.get("rerank_applied") is not False
        or not isinstance(events, list)
        or any(not isinstance(event.get("task_id"), str) for event in events)
        or not isinstance(native.get("lane_call_counts"), dict)
        or any(type(value) is not int for value in native["lane_call_counts"].values())
        or native.get("lane_call_counts") != {"bm25": len(events), "semantic": 0, "encode": 0}
        or len(measured) != len(expected)
        or {event.get("task_id") for event in measured} != set(expected)
        or any(
            event.get("lane_entry_counts") != {"bm25": 1, "semantic": 0}
            or any(type(value) is not int for value in event["lane_entry_counts"].values())
            or event.get("profile_sha256") != digest(canonical(profiles["semble"]))
            or event.get("submitted_query_sha256")
            != hashlib.sha256(expected.get(event.get("task_id"), ("", []))[0].encode()).hexdigest()
            for event in events
        )
    ):
        raise ValueError(
            "Semble native file execution events differ from the frozen lexical profile/query inventory"
        )
    metrics = rebuilt["judgment_metrics"]["file_judgments"]
    scored = {(row["task_id"], row["route"]): row for row in metrics["per_query"]}
    result = {}
    for route, label in (
        ("lexical", "quanta_lexical"),
        ("semble-lexical-file", "semble_lexical_file"),
    ):
        timing_layer = phases["quanta" if route == "lexical" else "semble"]["query_timing"][
            "boundary"
        ]
        rows = []
        for observation in merged["results"]:
            if observation["route"] != route:
                continue
            task_id = observation["task_id"]
            gold = bool(expected[task_id][1])
            judgments = scored.get((task_id, route))
            if judgments is None:
                if gold:
                    raise ValueError("answerable raw record lacks its replayed file judgments")
                judgments = {"eligible": False}
            scores = judgments.get("scores", {})
            rows.append(
                {
                    "task_id": task_id,
                    "route": route,
                    "status": observation["status"],
                    "answerable": gold,
                    "candidates": len(observation["candidates"]),
                    "paths": [row["path"] for row in observation["candidates"]],
                    "eligible": judgments["eligible"],
                    "file_hit_at_10": bool(scores["hit_at_10"])
                    if judgments["eligible"] and gold
                    else "not_applicable",
                    "file_recall_at_10": scores["recall_at_10"]
                    if judgments["eligible"] and gold
                    else "not_applicable",
                    "file_ndcg_at_10": scores["ndcg_at_10"]
                    if judgments["eligible"] and gold
                    else "not_applicable",
                    "query_latency_ms": observation["timings"]["query_latency_ms"],
                }
            )
        result[label] = {
            "tasks": len(expected),
            "rank_unit": "distinct_file",
            "execution_profile": profiles["quanta" if route == "lexical" else "semble"],
            "coverage": metrics["routes"][route]["coverage"],
            "judgment_metrics": metrics["routes"][route],
            "per_query": sorted(rows, key=lambda row: row["task_id"]),
            "latency_ms": latency_summary(
                [row["query_latency_ms"] for row in rows if row["query_latency_ms"] is not None],
                sum(row["query_latency_ms"] is not None for row in rows),
                timing_layer,
            )
            if any(row["query_latency_ms"] is not None for row in rows)
            else {"count": 0, "timing_layer": timing_layer, "state": "not_recorded"},
            "missing_latency_task_ids": sorted(
                row["task_id"] for row in rows if row["query_latency_ms"] is None
            ),
        }
    return {
        "routes": result,
        "raw_sha256": {role: hashlib.sha256(data).hexdigest() for role, data in raw.items()},
        "source_bundle_sha256": bundle.sha256,
        "runner_record_sha256": digest(canonical(merged)),
        "source_provenance": provenance,
        "qualification": "not_applicable",
        "source_scope": "retained_corpus_bundle_and_declared_original_binary_source_provenance",
    }


def read_spec(path: Path) -> dict[str, Path]:
    spec = _read(path)
    roles = input_roles(spec)
    if (
        set(spec) != {"schema_version", *roles}
        or type(spec["schema_version"]) is not int
        or spec["schema_version"] != 1
    ):
        raise ValueError("lexical spec requires schema_version 1 and the exact input roles")
    if any(
        not isinstance(spec[role], str)
        or not Path(spec[role]).is_absolute()
        or ".." in Path(spec[role]).parts
        for role in roles
    ):
        raise ValueError("lexical spec inputs must be explicit absolute canonical paths")
    return {role: Path(spec[role]) for role in roles}


def evaluate_capture(paths: dict[str, Path]) -> dict:
    """One scorer authority for the owner CLI and common capture/replay."""
    roles = input_roles(paths)
    if set(paths) != set(roles) or any(not isinstance(path, Path) for path in paths.values()):
        raise ValueError("lexical capture requires the exact input role inventory")
    suite_raw, pack_raw = _bytes(paths["suite"]), _bytes(paths["query_pack"])
    suite, pack = _json(suite_raw), _json(pack_raw)
    universe = _file_universe(suite, pack)
    file_policy = (
        _file_policy_from_lock(_json(_bytes(paths["pair_lock"])))
        if roles == FILE_INPUT_ROLES
        else "code_search_file"
    )
    expected = _tasks(suite, pack, file_policy=file_policy)
    if any(path not in universe for _, gold in expected.values() for path in gold):
        raise ValueError("gold path is outside the frozen file universe")
    result = {
        "status": "diagnostic_unqualified",
        "query_form": (
            "code_search_typo_identifier_v1"
            if file_policy == "code_search_typo_file"
            else "code_search_atoms_v1"
        )
        if suite["routes"] == FILE_ROUTES
        else "bare_symbol_v1",
        "metric": "gold_file_recall_in_native_top_10",
        "rank_unit_equivalence": "non_equivalent",
        "latency_interpretation": "descriptive_only_not_cross_product_comparable",
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "suite_sha256": hashlib.sha256(suite_raw).hexdigest(),
        "query_pack_sha256": hashlib.sha256(pack_raw).hexdigest(),
        "validator_sha256": _sha(Path(__file__)),
        "pair": file_pair_result(paths, suite_raw, pack_raw)
        if suite["routes"] == FILE_ROUTES
        else pair_result(
            paths["pair_report"],
            paths["pair_lock"],
            paths["semble_native"],
            paths["pair_verdict"],
            pack,
            suite,
            len(expected),
        ),
        "products": {
            name: product_result(
                name,
                paths[f"{name}_rows"],
                expected,
                universe,
                file_judgments={
                    task["task_id"]: task.get("file_judgments", []) for task in suite["tasks"]
                }
                if suite["routes"] == FILE_ROUTES
                else None,
            )
            for name in PRODUCTS
        },
        "exclusions": [
            "independent_gold",
            "native_rank_equivalence",
            "qualified_latency",
            "backend_indexed_universe_attestation",
        ],
    }
    # Ten native chunks projected to files and ten distinct files are different
    # result units: each group has its own denominator and is never ranked
    # against the other.
    groups: dict[str, list[str]] = {}
    for name, row in result["products"].items():
        groups.setdefault(row["rank_unit"], []).append(name)
    for route, row in result["pair"]["routes"].items():
        groups.setdefault(row["rank_unit"], []).append("pair:" + route)
    result["comparison_groups"] = {unit: sorted(names) for unit, names in sorted(groups.items())}
    result["cross_unit_comparison"] = "not_permitted"
    products = {**result["products"], **result["pair"]["routes"]}
    requested = set(expected)
    eligible = requested.copy()
    coverage = {}
    for name, product in products.items():
        observed = {
            row["task_id"]
            for row in product["per_query"]
            if row.get("status") != "unsupported" and row.get("eligible", True)
        }
        eligible &= observed
        coverage[name] = {
            "requested": len(requested),
            "eligible": len(observed),
            "ineligible_task_ids": sorted(requested - observed),
        }
    result["capability_coverage"] = coverage
    result["common_eligible_task_ids"] = sorted(eligible)
    result["common_eligible_tasks"] = len(eligible)
    result["common_denominator_policy"] = (
        "intersection_of_explicit_product_capability_and_judgment_eligibility"
    )
    result["common_eligible_products"] = {}
    for name, product in products.items():
        rows = [row for row in product["per_query"] if row["task_id"] in eligible]
        scored_rows = [row for row in rows if row["file_recall_at_10"] != "not_applicable"]
        metrics = {}
        for field in ("file_hit_at_10", "file_recall_at_10", "file_ndcg_at_10"):
            values = [
                row[field] for row in scored_rows if field in row and row[field] != "not_applicable"
            ]
            metrics[field] = math.fsum(values) / len(values) if values else "not_applicable"
        result["common_eligible_products"][name] = {
            "tasks": len(rows),
            "answerable_tasks": len(scored_rows),
            **metrics,
        }
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--spec", type=Path)
    for role in FILE_INPUT_ROLES:
        parser.add_argument("--" + role.replace("_", "-"), type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    explicit = {role: getattr(args, role) for role in FILE_INPUT_ROLES}
    try:
        if args.spec is not None:
            if any(path is not None for path in explicit.values()):
                parser.error("--spec refuses mixed explicit input controls")
            paths = read_spec(args.spec)
        else:
            explicit = {role: path for role, path in explicit.items() if path is not None}
            roles = input_roles(explicit)
            if any(explicit.get(role) is None for role in roles):
                parser.error("provide --spec or every explicit input role")
            paths = explicit
        if args.out.exists() or args.out.is_symlink():
            raise ValueError("output already exists")
        result = evaluate_capture(paths)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("x", encoding="utf-8") as output:
            output.write(json.dumps(result, indent=2, sort_keys=True) + "\n")
    except (ValueError, OSError) as exc:
        parser.error(str(exc))


if __name__ == "__main__":
    main()
