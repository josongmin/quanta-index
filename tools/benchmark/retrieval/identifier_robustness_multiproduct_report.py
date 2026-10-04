#!/usr/bin/env python3
"""Replay frozen C5 file results with source-defined typo strata.

The report reads existing product rows. It does not issue searches or change the
original suite, gold, census, capture, or five-product summary.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import statistics
import sys
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

try:
    from tools.benchmark.retrieval import evaluator, identifier_robustness_suite, source_oracle
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval import evaluator, identifier_robustness_suite, source_oracle

PRODUCTS = ("quanta", "semble", "sourcegraph", "cs", "opengrok")
INTENT = "declaration_name_osa1_casefold"
PAIR_REPORT = "report-semble-lexical-file-vs-lexical-fixed_window_strict.json"
SCORES = ("hit_at_10", "mrr_at_10", "ndcg_at_10")
FILE_METRICS = ("intended_name_file", "intended_original_file")
LABEL_CONTRACT = {
    "intended_name_file": "exact_original_name_declaration_files",
    "intended_original_file": "representative_original_source_file",
}


class OfflineReportError(ValueError):
    """Frozen inputs are incomplete or inconsistent."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise OfflineReportError(message)


def _read(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_bytes())
    _require(isinstance(value, dict), "expected JSON object: " + str(path))
    return value


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _canonical_sha(value: Any) -> str:
    return hashlib.sha256(evaluator.canonical(value)).hexdigest()


def _jsonl(path: Path) -> list[dict[str, Any]]:
    rows = [json.loads(line) for line in path.read_bytes().splitlines() if line.strip()]
    _require(all(isinstance(row, dict) for row in rows), "malformed JSONL: " + str(path))
    return rows


def _unique(rows: list[dict[str, Any]], key: str, where: str) -> dict[str, dict[str, Any]]:
    values = [row[key] for row in rows]
    _require(
        all(isinstance(value, str) and value for value in values)
        and len(values) == len(set(values)),
        "duplicate or invalid " + key + ": " + where,
    )
    return dict(zip(values, rows, strict=True))


def _score(paths: list[str], judgments: list[dict[str, Any]]) -> dict[str, float]:
    candidates = [{"path": path} for path in paths]
    return {
        "hit_at_10": evaluator.file_hit_at_k_judged(candidates, judgments, 10),
        "mrr_at_10": evaluator.file_mrr_at_k_judged(candidates, judgments, 10),
        "ndcg_at_10": evaluator.file_ndcg_at_k(candidates, judgments, 10),
    }


def _top10(paths: Any, universe: set[str], where: str) -> list[str]:
    _require(
        isinstance(paths, list)
        and len(paths) <= 10
        and all(isinstance(path, str) and path in universe for path in paths)
        and len(paths) == len(set(paths)),
        "invalid top-10 file paths: " + where,
    )
    return paths


def _task_source_strata(task: dict[str, Any]) -> dict[str, str]:
    submitted, intended = task["query"], task["intended_name"]
    _require(
        task["query_sha256"] == hashlib.sha256(submitted.encode("utf-8")).hexdigest()
        and task["evaluation_contract"]
        == {
            "request_mode": "default_file_search",
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
        and source_oracle.osa_distance_at_most_one(submitted.casefold(), intended.casefold()),
        "invalid source-bound typo task: " + task["task_id"],
    )
    return identifier_robustness_suite.typo_source_strata(intended, submitted)


def _result(
    task: dict[str, Any],
    paths: list[str],
    *,
    eligible: bool,
    status: str,
    latency_ms: float | None,
) -> dict[str, Any]:
    authority = task.get("source_oracle", {})
    _require(
        authority.get("unit") == "distinct_file"
        and source_oracle.NAME_CONTRACTS.get(authority.get("contract"), (None, None))[1] == "exact",
        "intended-name file metric requires exact declaration source authority",
    )
    _require(eligible or not paths, "failed result has partial top-10 candidates")
    _require(
        latency_ms is None
        or type(latency_ms) in (int, float)
        and math.isfinite(latency_ms)
        and latency_ms >= 0,
        "invalid query latency",
    )
    return {
        "status": status,
        "eligible": eligible,
        "top10_paths": paths,
        "latency_ms": latency_ms,
        "label_contract": dict(LABEL_CONTRACT),
        "intended_name_file": _score(paths, task["file_judgments"]),
        "intended_original_file": _score(paths, task["gold"]),
    }


def _mean(values: list[float]) -> float | None:
    return statistics.mean(values) if values else None


def _percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    point = (len(ordered) - 1) * fraction
    lower, upper = math.floor(point), math.ceil(point)
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (point - lower)


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    """Report operational and successful-call cohorts using evaluator scores."""
    eligible = [row for row in rows if row["eligible"]]
    times = [row["latency_ms"] for row in rows if row["latency_ms"] is not None]
    output = {
        "primary_metric": "intended_name_file",
        "label_contract": dict(LABEL_CONTRACT),
        "selected": len(rows),
        "eligible": len(eligible),
        "status_counts": dict(sorted(Counter(row["status"] for row in rows).items())),
        "timing_ms": {
            "count": len(times),
            "sum": sum(times),
            "p50": _percentile(times, 0.5),
            "p95": _percentile(times, 0.95),
        },
    }
    for label in FILE_METRICS:
        output[label] = {
            "operational": {
                metric: _mean([row[label][metric] for row in rows]) for metric in SCORES
            },
            "conditional": {
                metric: _mean([row[label][metric] for row in eligible]) for metric in SCORES
            },
            "hit_count": sum(int(row[label]["hit_at_10"]) for row in rows),
        }
    return output


def _cell_index(cells: list[dict[str, Any]], intent: str) -> dict[str, dict[str, Any]]:
    return _unique([cell for cell in cells if cell["intent"] == intent], "repository", intent)


def _paired_receipts(
    pair: dict[str, Any],
    pair_ledger: dict[str, Any],
    continuation: dict[str, Any],
    continuation_ledger: dict[str, Any],
    split_root: Path,
) -> dict[str, tuple[dict[str, Any], dict[str, Any], bool]]:
    _require(pair_ledger["manifest_sha256"] == _sha(Path(pair["_path"])), "pair manifest drift")
    _require(
        continuation_ledger["manifest_sha256"] == _sha(Path(continuation["_path"]))
        and continuation["continuation_of_sha256"] == _sha(Path(pair["_path"]))
        and continuation["source_head"] == pair["source_head"]
        and continuation["runner_sha256"] == pair["runner_sha256"]
        and continuation["searchd_sha256"] == pair["searchd_sha256"],
        "continuation identity differs",
    )
    pair_cells = _cell_index(pair["cells"], INTENT)
    completed: dict[str, tuple[dict[str, Any], bool]] = {}
    for manifest, ledger in ((pair, pair_ledger), (continuation, continuation_ledger)):
        prefix = manifest["cells"][: len(ledger["rows"])]
        for cell, receipt in zip(prefix, ledger["rows"], strict=True):
            if cell["intent"] != INTENT:
                continue
            repo = cell["repository"]
            _require(repo in pair_cells and repo not in completed, "duplicate pair cell")
            _require(
                (repo, cell["intent"], cell["tasks"], cell["spec_sha256"])
                == (
                    receipt["repository"],
                    receipt["intent"],
                    receipt["tasks"],
                    receipt["spec_sha256"],
                ),
                "pair ledger cell mismatch",
            )
            completed[repo] = receipt, False
    missing = set(pair_cells) - set(completed)
    _require(len(missing) == 1, "expected exactly one split pair cell")
    split_repo = next(iter(missing))
    precommit = _read(split_root / "precommit.json")
    _require(
        precommit["pair_manifest_sha256"] == _sha(Path(pair["_path"]))
        and precommit["suite_sha256"] == pair_cells[split_repo]["suite_sha256"]
        and precommit["blind_pack_sha256"] == pair_cells[split_repo]["blind_pack_sha256"]
        and precommit["original_spec_sha256"] == pair_cells[split_repo]["spec_sha256"],
        "split cell binding differs",
    )
    completed[split_repo] = {}, True
    return {
        repo: (pair_cells[repo], receipt, split) for repo, (receipt, split) in completed.items()
    }


def build(
    pair_root: Path,
    continuation_root: Path,
    split_root: Path,
    split_quanta_root: Path,
    external_root: Path,
) -> dict[str, Any]:
    pair, pair_ledger = _read(pair_root / "manifest.json"), _read(pair_root / "ledger.json")
    continuation = _read(continuation_root / "manifest.json")
    continuation_ledger = _read(continuation_root / "ledger.json")
    external, external_ledger = (
        _read(external_root / "manifest.json"),
        _read(external_root / "ledger.json"),
    )
    pair["_path"] = str(pair_root / "manifest.json")
    continuation["_path"] = str(continuation_root / "manifest.json")
    paired = _paired_receipts(pair, pair_ledger, continuation, continuation_ledger, split_root)
    _require(
        external_ledger["precommit_sha256"] == _sha(external_root / "manifest.json")
        and external_ledger["status"] == "diagnostic_unqualified"
        and external_ledger["release_revalidated_after_capture"] is True,
        "external capture custody differs",
    )
    external_cells = _cell_index(external["cells"], INTENT)
    external_receipts = _cell_index(external_ledger["rows"], INTENT)
    _require(set(paired) == set(external_cells) == set(external_receipts), "cell coverage differs")
    per_query: list[dict[str, Any]] = []
    bindings: list[dict[str, Any]] = []
    for repo in sorted(paired):
        cell, receipt, split = paired[repo]
        ext_cell = external_cells[repo]
        _require(
            (cell["suite_sha256"], cell["blind_pack_sha256"], cell["tasks"])
            == (
                ext_cell["suite_sha256"],
                ext_cell["blind_pack_sha256"],
                ext_cell["tasks"],
            ),
            "pair/external input differs: " + repo,
        )
        spec_path = Path(cell["spec_path"])
        _require(cell["spec_sha256"] == _sha(spec_path), "pair spec differs: " + repo)
        suite_path = Path(_read(spec_path)["suite"])
        _require(cell["suite_sha256"] == _sha(suite_path), "suite differs: " + repo)
        suite = _read(suite_path)
        tasks = _unique(suite["tasks"], "task_id", "suite " + repo)
        _require(len(tasks) == cell["tasks"], "suite task count differs: " + repo)
        universe = {item["path"] for item in suite["file_universe"]}
        _require(len(universe) == len(suite["file_universe"]), "duplicate universe file")
        for task in tasks.values():
            _require(
                task["category"] == INTENT
                and task["answerable"] is True
                and bool(task["gold"])
                and bool(task["file_judgments"])
                and all(row["path"] in universe for row in task["gold"] + task["file_judgments"]),
                "invalid typo gold: " + task["task_id"],
            )
        ext_root = Path(ext_cell["output_root"])
        _require(
            ext_cell["spec_sha256"] == _sha(Path(ext_cell["spec_path"]))
            and external_receipts[repo]["capture_summary_sha256"]
            == _sha(ext_root / "capture.json"),
            "external capture receipt differs: " + repo,
        )
        capture = _read(ext_root / "capture.json")
        _require(
            capture["status"] == "diagnostic_unqualified" and capture["tasks"] == len(tasks),
            "external capture incomplete: " + repo,
        )
        by_product: dict[str, dict[str, dict[str, Any]]] = {}
        pair_output = Path(cell["output_root"])
        if not split:
            _require(
                receipt["verdict_sha256"] == _sha(pair_output / "verdict.json")
                and receipt["report_sha256"] == _sha(pair_output / PAIR_REPORT),
                "pair receipt differs: " + repo,
            )
        else:
            split_precommit = _read(split_root / "precommit.json")
            split_receipt = _read(split_root / "semble-receipt.json")
            _require(
                split_precommit["failed_pair_ledger_sha256"] == _sha(pair_root / "ledger.json")
                and split_receipt["precommit_sha256"] == _sha(split_root / "precommit.json")
                and split_receipt["status"] == "captured"
                and split_receipt["exit_code"] == 0,
                "split receipt differs: " + repo,
            )
        native_record_sha256: dict[str, str] = {}
        for product, route in (("quanta", "lexical"), ("semble", "semble-lexical-file")):
            record_root = (
                (split_quanta_root if product == "quanta" else split_root) if split else pair_output
            )
            record_path = record_root / (
                "strategy-00-fixed_window_strict/record.json"
                if split and product == "quanta"
                else "semble/record.json"
                if split
                else "rep-00/quanta/strategy-00-fixed_window_strict/record.json"
                if product == "quanta"
                else "rep-00/semble/record.json"
            )
            pack_path = record_root / (
                "quanta-pack.json"
                if split and product == "quanta"
                else "semble-pack.json"
                if split
                else "rep-00/quanta/quanta-pack.json"
                if product == "quanta"
                else "semble-pack.json"
            )
            record = _read(record_path)
            native_record_sha256[product] = _sha(record_path)
            if split and product == "semble":
                _require(
                    split_receipt["record_sha256"] == native_record_sha256[product],
                    "split Semble record differs",
                )
            _require(
                record["query_pack_sha256"] == _canonical_sha(_read(pack_path)),
                "native pack differs: " + repo + "/" + product,
            )
            results = _unique(record["results"], "task_id", repo + "/" + product)
            _require(set(results) == set(tasks), "native task coverage differs")
            product_rows = {}
            for task_id, native in results.items():
                _require(
                    native["route"] == route and native["rank_unit"] == "distinct_file",
                    "native route/unit differs",
                )
                paths = _top10(
                    [item["path"] for item in native["candidates"][:10]],
                    universe,
                    repo + "/" + product + "/" + task_id,
                )
                status = native["status"]
                _require(status in evaluator.RESULT_STATUSES, "unknown native status")
                latency = None if split else native["timings"].get("query_latency_ms")
                product_rows[task_id] = _result(
                    tasks[task_id],
                    paths,
                    eligible=status != "error",
                    status=status,
                    latency_ms=latency,
                )
            by_product[product] = product_rows
        for product in ("sourcegraph", "cs", "opengrok"):
            rows_path = ext_root / (product + "_rows.jsonl")
            _require(capture["rows_sha256"][product] == _sha(rows_path), "raw rows differ")
            rows = _unique(_jsonl(rows_path), "task_id", repo + "/" + product)
            _require(set(rows) == set(tasks), "external task coverage differs")
            product_rows = {}
            for task_id, raw in rows.items():
                task = tasks[task_id]
                _require(
                    raw["submitted_query"] == task["query"]
                    and set(raw["gold_paths"])
                    == {item["path"] for item in task["gold"] if item["grade"] > 0},
                    "external query/gold differs: " + repo + "/" + product + "/" + task_id,
                )
                paths = _top10(
                    raw["paths"][:10] if product == "cs" else raw["file_paths_top_10"],
                    universe,
                    repo + "/" + product + "/" + task_id,
                )
                eligible = (
                    raw["exit_code"] == 0
                    if product == "cs"
                    else raw["http_status"] == 200 and raw["error"] is None
                )
                observed_hit = _score(paths, task["gold"])["hit_at_10"]
                _require(
                    bool(observed_hit) == raw["file_hit_at_10"],
                    "external recorded hit differs",
                )
                product_rows[task_id] = _result(
                    task,
                    paths,
                    eligible=eligible,
                    status="success" if eligible else "failed_or_unsupported",
                    latency_ms=raw["elapsed_ms"],
                )
            by_product[product] = product_rows
        bindings.append(
            {
                "repository": repo,
                "suite_sha256": cell["suite_sha256"],
                "task_count": len(tasks),
                "split_protocol": split,
                "native_record_sha256": native_record_sha256,
                "external_rows_sha256": capture["rows_sha256"],
            }
        )
        for task_id, task in tasks.items():
            per_query.append(
                {
                    "repository": repo,
                    "task_id": task_id,
                    "query_family_id": task["query_family_id"],
                    "submitted_query": task["query"],
                    "intended_name": task["intended_name"],
                    "source_strata": _task_source_strata(task),
                    "products": {product: by_product[product][task_id] for product in PRODUCTS},
                }
            )
    groups: dict[str, dict[str, list[dict[str, Any]]]] = defaultdict(
        lambda: {product: [] for product in PRODUCTS}
    )
    for row in per_query:
        for group in (
            "overall",
            "literal_relation/" + row["source_strata"]["literal_relation"],
            "surviving_components/" + row["source_strata"]["surviving_components"],
            "joint/"
            + row["source_strata"]["literal_relation"]
            + "/"
            + row["source_strata"]["surviving_components"],
        ):
            for product in PRODUCTS:
                groups[group][product].append(row["products"][product])
    summaries = {
        group: {product: summarize(rows) for product, rows in products.items()}
        for group, products in sorted(groups.items())
    }
    return {
        "schema": "identifier_robustness_five_product_offline_strata_v2",
        "primary_metric": "intended_name_file",
        "label_contract": dict(LABEL_CONTRACT),
        "status": "diagnostic_unqualified",
        "execution": "offline_replay_of_existing_captures",
        "native_split_timing_scope": "split_cell_query_timings_excluded",
        "intent": INTENT,
        "source_strata_policy": identifier_robustness_suite.TYPO_SOURCE_STRATA_POLICY,
        "report_tool_sha256": _sha(Path(__file__)),
        "strata_generator_sha256": _sha(Path(identifier_robustness_suite.__file__)),
        "evaluator_sha256": _sha(Path(evaluator.__file__)),
        "producer_source_head": pair["source_head"],
        "external_adapter_source_head": external["source_head"],
        "input_sha256": {
            "pair_manifest": _sha(pair_root / "manifest.json"),
            "pair_ledger": _sha(pair_root / "ledger.json"),
            "continuation_manifest": _sha(continuation_root / "manifest.json"),
            "continuation_ledger": _sha(continuation_root / "ledger.json"),
            "split_precommit": _sha(split_root / "precommit.json"),
            "external_manifest": _sha(external_root / "manifest.json"),
            "external_ledger": _sha(external_root / "ledger.json"),
        },
        "cell_bindings": bindings,
        "task_count": len(per_query),
        "summaries": summaries,
        "per_query": per_query,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for name in (
        "pair_root",
        "continuation_root",
        "split_root",
        "split_quanta_root",
        "external_root",
        "output",
    ):
        parser.add_argument("--" + name.replace("_", "-"), required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    checkout = Path(__file__).resolve().parents[3]
    _require(output.is_absolute() and not output.exists(), "output must be a new absolute file")
    _require(not output.is_relative_to(checkout), "output must be outside source checkout")
    result = build(
        args.pair_root,
        args.continuation_root,
        args.split_root,
        args.split_quanta_root,
        args.external_root,
    )
    with output.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, ensure_ascii=False, sort_keys=True, indent=2, allow_nan=False)
        stream.write("\n")
    print(
        json.dumps(
            {"status": result["status"], "tasks": result["task_count"], "sha256": _sha(output)},
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
