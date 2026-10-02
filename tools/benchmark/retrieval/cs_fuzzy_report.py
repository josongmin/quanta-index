"""Score captured cs fuzzy responses on a conservative compatible edit subset.

The cs 3.2.0 `~1` capture uses its native fuzzy term syntax. An adjacent
transposition is one OSA edit but two Levenshtein edits, and the native probe
does not return that case at distance one. Keep those rows visible without
promoting them to an equivalent one-edit product comparison.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from tools.benchmark.retrieval import evaluator, live_lexical_external


def levenshtein_one(first: str, second: str) -> bool:
    """Recognize exactly one insertion, deletion, or substitution after folding."""
    first, second = first.casefold(), second.casefold()
    if abs(len(first) - len(second)) > 1 or first == second:
        return False
    if len(first) == len(second):
        return sum(left != right for left, right in zip(first, second)) == 1
    if len(first) > len(second):
        first, second = second, first
    index = 0
    while index < len(first) and first[index] == second[index]:
        index += 1
    return first[index:] == second[index + 1 :]


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
    unsupported = []
    raw_hit = 0
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
        if levenshtein_one(task["query"], task["intended_name"]):
            compatible.append({"task_id": row["task_id"], "scores": scores})
        else:
            unsupported.append(row["task_id"])
    totals = {
        metric: sum(row["scores"][metric] for row in compatible)
        for metric in ("hit_at_10", "mrr_at_10", "ndcg_at_10")
    }
    return {
        "schema_version": 1,
        "qualification": "diagnostic_unqualified",
        "legacy_capture_capability": summary["capability"],
        "native_request": "cs 3.2.0 fuzzy term ~1",
        "compatible_edit_contract": "casefold_levenshtein_distance_exactly_one",
        "capture_sha256": evaluator.digest((root / "capture.json").read_bytes()),
        "suite_sha256": evaluator.digest((root / "suite.json").read_bytes()),
        "submitted": len(rows),
        "compatible": len(compatible),
        "unsupported": len(unsupported),
        "unsupported_task_ids": sorted(unsupported),
        "raw_hit_at_10_count": int(raw_hit),
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
