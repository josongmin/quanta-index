"""Score cs 3.2.0 fuzzy responses by edit operation.

Its `~1` matcher applies Levenshtein to same-length content windows. That
supports one substitution at an identifier's original length; insertion,
deletion and transposition are not equivalent to Quanta's OSA1 operation on a
whole identifier. Preserve their native outcomes, but keep them out of the
shared edit-class score. Search targets and ranking remain product-specific.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from tools.benchmark.retrieval import evaluator, live_lexical_external


def _one_gap(shorter: str, longer: str) -> bool:
    index = 0
    while index < len(shorter) and shorter[index] == longer[index]:
        index += 1
    return shorter[index:] == longer[index + 1 :]


def edit_operation(query: str, intended_name: str) -> str:
    """Classify an ASCII identifier mutation independently of retrieved rows."""
    if not query.isascii() or not intended_name.isascii():
        return "non_ascii"
    query, intended_name = query.casefold(), intended_name.casefold()
    if query == intended_name:
        return "unchanged"
    if len(query) == len(intended_name):
        different = [
            index for index, (left, right) in enumerate(zip(query, intended_name)) if left != right
        ]
        if len(different) == 1:
            return "substitution"
        if (
            len(different) == 2
            and different[1] == different[0] + 1
            and query[different[0]] == intended_name[different[1]]
            and query[different[1]] == intended_name[different[0]]
        ):
            return "transposition"
    elif len(query) == len(intended_name) + 1 and _one_gap(intended_name, query):
        return "insertion"
    elif len(query) + 1 == len(intended_name) and _one_gap(query, intended_name):
        return "deletion"
    return "other"


def score_capture(root: Path) -> dict:
    """Replay the native capture before computing source-bound file scores."""
    summary = live_lexical_external.verify_cs_fuzzy(root)
    suite = evaluator.read_json(root / "suite.json")
    tasks = {task["task_id"]: task for task in suite["tasks"]}
    rows = [json.loads(line) for line in (root / "cs_fuzzy_rows.jsonl").read_text().splitlines()]
    evaluator.require(
        len(tasks) == len(rows) and set(tasks) == {row["task_id"] for row in rows},
        "cs fuzzy scored rows differ from source-bound suite",
    )
    compatible = []
    excluded = []
    raw_hit = 0
    by_edit: dict[str, dict[str, float | int]] = {}
    for row in rows:
        task = tasks[row["task_id"]]
        candidates = [{"path": path} for path in row["paths"]]
        judgments = task["file_judgments"]
        scores = {
            "hit_at_10": evaluator.file_hit_at_k_judged(candidates, judgments, 10),
            "mrr_at_10": evaluator.file_mrr_at_k_judged(candidates, judgments, 10),
            "ndcg_at_10": evaluator.file_ndcg_at_k(candidates, judgments, 10),
        }
        raw_hit += scores["hit_at_10"]
        operation = edit_operation(task["query"], task["intended_name"])
        operation_row = by_edit.setdefault(
            operation,
            {"submitted": 0, "hit_at_10_count": 0, "mrr_at_10_sum": 0.0, "ndcg_at_10_sum": 0.0},
        )
        operation_row["submitted"] += 1
        operation_row["hit_at_10_count"] += int(scores["hit_at_10"])
        operation_row["mrr_at_10_sum"] += scores["mrr_at_10"]
        operation_row["ndcg_at_10_sum"] += scores["ndcg_at_10"]
        if operation == "substitution":
            compatible.append({"task_id": row["task_id"], "scores": scores})
        else:
            excluded.append(row["task_id"])
    totals = {
        metric: sum(row["scores"][metric] for row in compatible)
        for metric in ("hit_at_10", "mrr_at_10", "ndcg_at_10")
    }
    return {
        "schema_version": 1,
        "qualification": "diagnostic_unqualified",
        "legacy_capture_capability": summary["capability"],
        "native_request": "cs 3.2.0 fuzzy term ~1",
        "compatible_edit_contract": "ascii_casefold_single_substitution_same_length_window",
        "capture_sha256": evaluator.digest((root / "capture.json").read_bytes()),
        "suite_sha256": evaluator.digest((root / "suite.json").read_bytes()),
        "submitted": len(rows),
        "compatible": len(compatible),
        "excluded_from_shared_edit_class": len(excluded),
        "excluded_task_ids": sorted(excluded),
        "raw_hit_at_10_count": int(raw_hit),
        "native_by_edit_operation": {key: by_edit[key] for key in sorted(by_edit)},
        "compatible_hit_at_10_count": int(totals["hit_at_10"]),
        "compatible_mrr_at_10": totals["mrr_at_10"] / len(compatible) if compatible else None,
        "compatible_ndcg_at_10": totals["ndcg_at_10"] / len(compatible) if compatible else None,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    print(json.dumps(score_capture(args.root), indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
