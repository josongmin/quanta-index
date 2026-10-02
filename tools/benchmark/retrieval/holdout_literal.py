"""Admit B08 raw-content gold to a Quanta-only CodeSearch diagnostic.

This validates the frozen capsule and source through the existing C4 binder,
then checks whether one case-sensitive content literal has the same file set as
the capsule's raw UTF-8 gold. It does not capture products or claim a default
CodeSearch benchmark score; the Rust runner has no bound profile for this lane.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import unicodedata2

from tools.benchmark.retrieval import evaluator, gold_oracle, holdout_c4, query_plan

INTENT = "literal_utf8_exact"


def _derive_prepared(prepared: holdout_c4._Prepared) -> dict:
    gold_tasks, blind_tasks = prepared.gold["tasks"], prepared.blind["tasks"]
    if len(gold_tasks) != len(blind_tasks):
        raise ValueError("literal gold/blind task counts differ")

    # A non-NFC source can produce extra CodeSearch matches, because its index
    # normalizes text while the frozen gold scans original UTF-8 bytes.
    changed_content: list[tuple[bytes, bytes]] = []
    invalid_utf8 = False
    for raw in prepared.files.values():
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            invalid_utf8 = True
            continue
        normalized = unicodedata2.normalize("NFC", text).encode("utf-8")
        if normalized != raw:
            changed_content.append((raw, normalized))

    admitted, excluded = [], []
    for task, public in zip(gold_tasks, blind_tasks, strict=True):
        if any(
            task.get(field) != public.get(field) for field in gold_oracle.TASK_FIELDS - {"split"}
        ):
            raise ValueError("literal blind task differs from gold recipe")
        if task["intent"] != INTENT:
            continue
        task_id, raw_query = task["task_id"], task["query"]
        if (
            task.get("label_state") != "mechanical_unreviewed"
            or task.get("unsupported") != []
            or task.get("case_semantics") != "sensitive"
            or task.get("normalization") != "none_raw_utf8"
            or task.get("scope_prefix") != ""
            or task.get("language") is not None
            or task.get("answerable") is not True
        ):
            excluded.append({"task_id": task_id, "reason": "unjudged_or_unsupported"})
            continue
        try:
            request = query_plan.plan_code_search_exact_content_request(raw_query)
        except query_plan.QueryPlanError:
            excluded.append({"task_id": task_id, "reason": "query_not_admitted"})
            continue
        needle = raw_query.encode("utf-8")
        expected = [
            (path, start, end, kind, prepared.source.file(path)[2])
            for path, content in prepared.files.items()
            for start, end, kind in gold_oracle._literal_spans(content, needle)
        ]
        observed = [
            (
                label["path"],
                label["start_byte"],
                label["end_byte"],
                label["kind"],
                label["file_sha256"],
            )
            for label in task["labels"]
        ]
        if sorted(observed) != sorted(expected) or bool(expected) != task["answerable"]:
            raise ValueError(f"literal labels differ from bound source: {task_id}")
        if invalid_utf8 or any(
            (needle in before) != (needle in after) for before, after in changed_content
        ):
            excluded.append(
                {"task_id": task_id, "reason": "normalized_content_differs_from_raw_gold"}
            )
            continue
        admitted.append(
            {
                "task_id": task_id,
                "query_sha256": hashlib.sha256(needle).hexdigest(),
                "effective_request": request,
                "effective_request_sha256": query_plan.code_search_effective_request_sha256(
                    request
                ),
                "gold_file_count": len({row[0] for row in expected}),
            }
        )
    return {
        "status": "diagnostic_unqualified",
        "product_capture": False,
        "qualified_default_search_conformance": False,
        "repository": prepared.selection["repository"],
        "intent": INTENT,
        "product": "quanta",
        "syntax": "code_search",
        "result_unit": "distinct_file",
        "query_contract": "case_sensitive_exact_content_literal",
        "runner_policy": None,
        "gold_capsule_identity_sha256": prepared.identity_sha256,
        "release_digest": prepared.document["digest"],
        "repository_commit": prepared.manifest["repository_commit"],
        "file_universe_digest": evaluator.universe_digest(prepared.manifest["files"]),
        "selected": len(admitted),
        "admitted": admitted,
        "excluded": excluded,
    }


def derive(release: Path, capsule: Path, checkout: Path) -> dict:
    """Validate one frozen repository and return an admission-only ledger."""
    prepared = holdout_c4._prepare(release, capsule, checkout)
    report = _derive_prepared(prepared)
    fresh = evaluator.SourceSnapshot(checkout, prepared.manifest["repository_commit"])
    evaluator.validate_file_universe(fresh, prepared.manifest["files"])
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--capsule", type=Path, required=True)
    parser.add_argument("--checkout", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(derive(args.release, args.capsule, args.checkout), sort_keys=True))


if __name__ == "__main__":
    main()
