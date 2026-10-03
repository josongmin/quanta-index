"""Admit frozen exact-content tasks to a Quanta-only file diagnostic.

The capsule producer's occurrence labels are rederived by a separate exact-byte
oracle. Product-normalized content must select the same files, but the raw byte
spans remain the gold contract. No human relevance or product score is claimed.
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import sys
from pathlib import Path

for path in (Path(__file__).resolve().parents[3], Path(__file__).resolve().parents[1]):
    if str(path) not in sys.path:
        sys.path.insert(0, str(path))

from tools.benchmark import corpus_binding  # noqa: E402
from tools.benchmark.evidence import digest_bytes  # noqa: E402
from tools.benchmark.retrieval import (  # noqa: E402
    evaluator,
    gold_oracle,
    holdout_c4,
    literal_source_oracle,
    query_plan,
)

INTENT = "literal_utf8_exact"
POLICY = "code_search_exact_content_file"


def _derive_prepared(prepared: holdout_c4._Prepared) -> tuple[dict, dict, dict]:
    gold_tasks = prepared.gold["tasks"]
    blind_tasks = prepared.blind["tasks"]
    name = prepared.selection["repository"]
    eligible_queries = set()
    for task in gold_tasks:
        if task["intent"] != INTENT:
            continue
        try:
            literal_source_oracle.require_literal(task["query"])
        except literal_source_oracle.LiteralOracleError:
            continue
        eligible_queries.add(task["query"])
    if not eligible_queries:
        raise ValueError("literal capsule has no admitted exact-content task")
    oracle = literal_source_oracle.LiteralSourceOracleIndex(
        {path: (raw, prepared.source.file(path)[2]) for path, raw in prepared.files.items()},
        eligible_queries,
    )
    selected = []
    excluded = []
    for task, public in zip(gold_tasks, blind_tasks, strict=True):
        if not isinstance(task, dict) or not isinstance(public, dict):
            raise ValueError("literal capsule task is malformed")
        if set(public) != gold_oracle.TASK_FIELDS - {"split"} or any(
            task.get(key) != public[key] for key in public
        ):
            raise ValueError("literal blind task differs from gold query")
        if task["intent"] != INTENT:
            continue
        if (
            task.get("label_state") != "mechanical_unreviewed"
            or task.get("unsupported") != []
            or task.get("language") is not None
            or task.get("case_semantics") != "sensitive"
            or task.get("normalization") != "none_raw_utf8"
            or task.get("scope_prefix") != ""
            or task.get("answerable") is not True
        ):
            raise ValueError(f"literal task is unjudged or unsupported: {task['task_id']}")
        if task["query"] not in eligible_queries:
            excluded.append(
                {"task_id": task["task_id"], "reason": "outside_literal_query_contract"}
            )
            continue
        try:
            query_plan.plan_lexical_request(POLICY, task["query"])
        except query_plan.QueryPlanError:
            excluded.append({"task_id": task["task_id"], "reason": "query_not_admitted"})
            continue
        spans = oracle.spans(task["query"])
        observed = [
            (label["path"], label["start_byte"], label["end_byte"]) for label in task["labels"]
        ]
        if sorted(observed) != spans or not spans:
            raise ValueError(
                f"literal producer labels differ from independent oracle: {task['task_id']}"
            )
        for label in task["labels"]:
            if (
                label["kind"] != "literal_occurrence"
                or label["file_sha256"] != prepared.source.file(label["path"])[2]
            ):
                raise ValueError(f"literal label kind or file hash differs: {task['task_id']}")
        if not oracle.indexed_nfc_membership_matches(task["query"]):
            excluded.append(
                {"task_id": task["task_id"], "reason": "raw_indexed_file_membership_differs"}
            )
            continue
        try:
            for prior in selected:
                evaluator.check_query_near_duplicates(
                    [
                        (prior["task_id"], prior["query"]),
                        (task["task_id"], task["query"]),
                    ]
                )
        except evaluator.EvidenceError:
            excluded.append({"task_id": task["task_id"], "reason": "query_near_duplicate"})
            continue
        selected.append(task)
    if not selected:
        raise ValueError("literal capsule has no admitted exact-content task")
    rows = []
    for task in selected:
        query = task["query"]
        rows.append(
            {
                "task_id": task["task_id"],
                "query": query,
                "query_sha256": evaluator.digest(query.encode()),
                "query_family_id": task["query_family_id"],
                "split": "eval",
                "category": INTENT,
                "query_intent": "exact_content",
                "source_oracle": {
                    "contract": literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT,
                    "unit": "distinct_file",
                },
                "judgment_policy": evaluator.SOURCE_ORACLE_JUDGMENT_POLICY,
                "file_judgments": oracle.expected_rows(
                    literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT, query, "distinct_file"
                ),
                "gold": evaluator.source_oracle_gold(
                    prepared.source, oracle, literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT, query
                ),
                "answerable": True,
            }
        )
    universe = prepared.manifest["files"]
    suite = {
        "schema_version": evaluator.SCHEMA_VERSION,
        "suite_id": f"{name}-{prepared.manifest['repository_commit'][:8]}-{INTENT}-code-search-file",
        "repository_commit": prepared.manifest["repository_commit"],
        "comparison_contract": {
            "top_k": 10,
            "tokenizer": evaluator.TOKENIZER,
            "tokenizer_budget_version": evaluator.TOKENIZER_BUDGET_VERSION,
            "output_unit_policy": "rank_prefix",
            "span_unit": evaluator.SPAN_UNIT,
        },
        "routes": ["lexical"],
        "file_universe": universe,
        "file_universe_digest": evaluator.universe_digest(universe),
        "diagnostic_policy": evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
        "tasks": rows,
    }
    checked, pack, _ = evaluator.validate_suite(prepared.checkout, suite)
    binding = corpus_binding._bind(
        prepared.document,
        prepared.manifest_raw,
        {
            "release_path": str(prepared.release.resolve()),
            "release_digest": prepared.document["digest"],
            "repository": name,
            "intent": INTENT,
            "view": "code_only",
        },
        evaluator.canonical(checked),
        evaluator.canonical(pack),
    )
    report = {
        "status": "diagnostic_unqualified",
        "repository": name,
        "selected": len(selected),
        "selected_task_ids": [task["task_id"] for task in selected],
        "excluded": excluded,
        "relevance_contract": literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT,
        "execution_policy": POLICY,
        "routes": ["lexical"],
        "qualified_default_search_conformance": False,
        "product_capture": False,
        "gold_capsule_identity_sha256": prepared.identity_sha256,
        "literal_oracle_source_sha256": digest_bytes(
            Path(literal_source_oracle.__file__).read_bytes()
        ),
        "evaluator_source_sha256": digest_bytes(Path(evaluator.__file__).read_bytes()),
        "binding": binding,
    }
    return checked, pack, report


def derive(release: Path, capsule: Path, checkout: Path) -> tuple[dict, dict, dict]:
    return _derive_prepared(holdout_c4._prepare(release, capsule, checkout))


def derive_batch(
    release: Path, capsule_root: Path, checkout_root: Path, *, expected_repositories: int = 12
) -> tuple[dict, dict[str, tuple[dict, dict, dict]]]:
    """Replay the global split once, then admit every repository's literal lane."""
    tool_sources = {
        **holdout_c4._batch_tool_sources(),
        "holdout_literal": Path(__file__),
        "literal_source_oracle": Path(literal_source_oracle.__file__),
    }
    source_digests = {name: digest_bytes(path.read_bytes()) for name, path in tool_sources.items()}
    batch = holdout_c4._batch_preflight(release, capsule_root, checkout_root, expected_repositories)
    payloads = {}
    cells = []
    prepared_rows = []
    for name in batch.names:
        prepared = holdout_c4._batch_prepare(batch, name)
        suite, pack, admission = _derive_prepared(prepared)
        prepared_rows.append(prepared)
        payloads[name] = (suite, pack, admission)
        cells.append(
            {
                "repository": name,
                "selected_task_ids": admission["selected_task_ids"],
                "selected": admission["selected"],
                "excluded": admission["excluded"],
                "gold_capsule_identity_sha256": prepared.identity_sha256,
                "suite_sha256": hashlib.sha256(evaluator.canonical(suite)).hexdigest(),
                "blind_pack_sha256": hashlib.sha256(evaluator.canonical(pack)).hexdigest(),
            }
        )
    holdout_c4._batch_recheck(batch, prepared_rows, tool_sources, source_digests)
    matrix = {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "repository_count": len(batch.names),
        "selected": sum(cell["selected"] for cell in cells),
        "product_capture": False,
        "qualified_default_search_conformance": False,
        "release_digest": batch.document["digest"],
        "split_manifest_sha256": hashlib.sha256(batch.split_raw).hexdigest(),
        "tool_source_sha256": source_digests,
        "cells": cells,
    }
    return matrix, payloads


def write_batch(
    release: Path,
    capsule_root: Path,
    checkout_root: Path,
    output: Path,
    *,
    expected_repositories: int = 12,
) -> dict:
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("literal output root must be fresh and absolute")
    target = output.resolve()
    protected = (Path(__file__).resolve().parents[3], release, capsule_root, checkout_root)
    if any(
        target.is_relative_to(root.resolve()) or root.resolve().is_relative_to(target)
        for root in protected
    ):
        raise ValueError("literal output root must be external and disjoint")
    matrix, payloads = derive_batch(
        release, capsule_root, checkout_root, expected_repositories=expected_repositories
    )
    output.mkdir(parents=True)
    try:
        (output / "literal-matrix.json").write_bytes(evaluator.canonical(matrix))
        for name, (suite, pack, admission) in payloads.items():
            target = output / name
            target.mkdir()
            for file_name, value in (
                ("suite.json", suite),
                ("blind-pack.json", pack),
                ("admission.json", admission),
            ):
                (target / file_name).write_bytes(evaluator.canonical(value))
    except BaseException:
        shutil.rmtree(output)
        raise
    return matrix


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--capsules", type=Path, required=True)
    parser.add_argument("--checkouts", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expected-repositories", type=int, default=12)
    args = parser.parse_args()
    matrix = write_batch(
        args.release,
        args.capsules,
        args.checkouts,
        args.output,
        expected_repositories=args.expected_repositories,
    )
    print(
        f"admitted {matrix['selected']} literal tasks from {matrix['repository_count']} repositories"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
