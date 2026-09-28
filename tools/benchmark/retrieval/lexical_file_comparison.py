#!/usr/bin/env python3
"""Validate one bare-symbol diagnostic across code-search products.

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
from pathlib import Path

from tools.benchmark.evidence import (
    CONTROL_DOCUMENT_BYTES,
    RawFile,
    _read_control_file,
    file_digest,
)
from tools.benchmark.retrieval.evaluator import canonical, digest
from tools.benchmark.retrieval.finite_json import is_finite_json_number
from tools.benchmark.retrieval.query_plan import execution_profile

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
TIMING_LAYERS = {
    "sourcegraph": "loopback_stream_http_request_wall",
    "opengrok": "loopback_rest_http_request_wall",
    "cs": "process_spawn_and_search_wall",
    "quanta_lexical": "runner_sdk_query_call",
    "semble_lexical_only": "worker_search_dispatch_call",
}


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
        and all(part not in ("", ".", "..") and part.casefold() != ".git" for part in path.split("/"))
    )


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
            or BARE_SYMBOL.fullmatch(query) is None
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
        if (
            not isinstance(gold, list)
            or not gold
            or any(
                not isinstance(label, dict)
                or not isinstance(label.get("path"), str)
                or not label["path"]
                for label in gold
            )
        ):
            raise ValueError("this diagnostic requires answerable file labels")
        paths = sorted({label["path"] for label in gold})
        expected[task_id] = task["query"], paths
    return expected


def product_result(
    product: str, path: Path, expected: dict[str, tuple[str, list[str]]], universe: set[str]
) -> dict:
    if product not in PRODUCTS:
        raise ValueError(f"unknown lexical product: {product}")

    if not expected or any(
        not isinstance(gold, list)
        or not gold
        or any(not isinstance(value, str) or not value for value in gold)
        or len(gold) != len(set(gold))
        for _, gold in expected.values()
    ):
        raise ValueError(f"{product}: empty or duplicate golden file inventory")
    raw = RawFile.capture(path)

    def consume(lines):
        seen: set[str] = set()
        hits = metadata_bytes = 0
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
                or any(not _canonical_result_path(value) or value not in universe for value in paths)
                or len(paths) != len(set(paths))
            ):
                raise ValueError(f"{product}: {task_id} malformed result paths")
            hit = bool(set(paths) & set(gold))
            if row.get("file_hit_at_10") is not hit:
                raise ValueError(f"{product}: {task_id} hit flag differs from paths")
            hits += hit
            elapsed.append(row.get("elapsed_ms"))
            per_query.append(
                {
                    "task_id": task_id,
                    "file_hit_at_10": hit,
                    "file_recall_at_10": len(set(paths) & set(gold)) / len(gold),
                    "query_latency_ms": row.get("elapsed_ms"),
                }
            )
            metadata_bytes += len(canonical(per_query[-1]))
            if metadata_bytes > CONTROL_DOCUMENT_BYTES:
                raise ValueError("lexical result metadata exceeds explicit control byte limit")
        if len(seen) != len(expected):
            raise ValueError(f"{product}: incomplete symbol-only lane")
        return hits, elapsed, per_query

    hits, elapsed, per_query = raw.consume_lines(consume)
    return {
        "hits": hits,
        "tasks": len(expected),
        "file_hit_rate_at_10": hits / len(expected),
        "file_recall_at_10": math.fsum(row["file_recall_at_10"] for row in per_query)
        / len(expected),
        "per_query": sorted(per_query, key=lambda row: row["task_id"]),
        "latency_ms": latency_summary(elapsed, len(expected), TIMING_LAYERS[product]),
        "raw_sha256": raw.sha256.removeprefix("sha256:"),
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
    raw = [_bytes(value) for value in (path, lock_path, native_path, verdict_path)]
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
        if type(recall) not in (int, float) or not 0 <= recall <= 1:
            raise ValueError(f"pair report {route} file recall is invalid")
        route_rows = [row for row in per_query if row.get("route") == route]
        if len(route_rows) != task_count or {row.get("task_id") for row in route_rows} != {
            task["task_id"] for task in pack["tasks"]
        }:
            raise ValueError(f"pair report {route} per-query tasks differ")
        recalls = [row.get("file_recall_at_10") for row in route_rows]
        flags = [row.get("file_hit_at_10") for row in route_rows]
        if (
            any(not is_finite_json_number(value) or not 0 <= value <= 1 for value in recalls)
            or any(type(value) is not bool for value in flags)
            or any(flag != (value > 0) for flag, value in zip(flags, recalls, strict=True))
            or abs(math.fsum(recalls) / task_count - recall) > 1e-10
        ):
            raise ValueError(f"pair report {route} recall/hit observations differ from aggregate")
        hits = sum(flags)
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
            "file_recall_at_10": recall,
            "file_hit_rate_at_10": hits / task_count,
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


def read_spec(path: Path) -> dict[str, Path]:
    spec = _read(path)
    if (
        set(spec) != {"schema_version", *INPUT_ROLES}
        or type(spec["schema_version"]) is not int
        or spec["schema_version"] != 1
    ):
        raise ValueError("lexical spec requires schema_version 1 and the exact input roles")
    if any(
        not isinstance(spec[role], str)
        or not Path(spec[role]).is_absolute()
        or ".." in Path(spec[role]).parts
        for role in INPUT_ROLES
    ):
        raise ValueError("lexical spec inputs must be explicit absolute canonical paths")
    return {role: Path(spec[role]) for role in INPUT_ROLES}


def evaluate_capture(paths: dict[str, Path]) -> dict:
    """One scorer authority for the owner CLI and common capture/replay."""
    if set(paths) != set(INPUT_ROLES) or any(not isinstance(path, Path) for path in paths.values()):
        raise ValueError("lexical capture requires the exact input role inventory")
    suite_raw, pack_raw = _bytes(paths["suite"]), _bytes(paths["query_pack"])
    suite, pack = _json(suite_raw), _json(pack_raw)
    universe = _file_universe(suite, pack)
    expected = _tasks(suite, pack)
    if any(path not in universe for _, gold in expected.values() for path in gold):
        raise ValueError("gold path is outside the frozen file universe")
    result = {
        "status": "diagnostic_unqualified",
        "query_form": "bare_symbol_v1",
        "metric": "file_recall_at_10",
        "latency_interpretation": "descriptive_only_not_cross_product_comparable",
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "suite_sha256": hashlib.sha256(suite_raw).hexdigest(),
        "query_pack_sha256": hashlib.sha256(pack_raw).hexdigest(),
        "validator_sha256": _sha(Path(__file__)),
        "pair": pair_result(
            paths["pair_report"],
            paths["pair_lock"],
            paths["semble_native"],
            paths["pair_verdict"],
            pack,
            suite,
            len(expected),
        ),
        "products": {
            name: product_result(name, paths[f"{name}_rows"], expected, universe)
            for name in PRODUCTS
        },
        "exclusions": [
            "independent_gold",
            "native_rank_equivalence",
            "qualified_latency",
            "backend_indexed_universe_attestation",
        ],
    }
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--spec", type=Path)
    for role in INPUT_ROLES:
        parser.add_argument("--" + role.replace("_", "-"), type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    explicit = {role: getattr(args, role) for role in INPUT_ROLES}
    try:
        if args.spec is not None:
            if any(path is not None for path in explicit.values()):
                parser.error("--spec refuses mixed explicit input controls")
            paths = read_spec(args.spec)
        else:
            if any(path is None for path in explicit.values()):
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
