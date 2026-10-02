"""Prepare two blind, unjudged file-review forms from an existing query pack.

This is a preparation adapter, not a labeler or a qualification pipeline.
Candidates from retrieval, source alternatives and random controls are pooled
by file. Product identities and pool membership stay in the owner-only custody
document; scores and ranks are rejected. Human decisions must subsequently enter the
existing evaluator judgments and run.py annotation/adjudication receipts.
"""

from __future__ import annotations

import hashlib
import shutil
from pathlib import Path

from tools.benchmark.retrieval import evaluator, source_oracle

POOL_KINDS = ("retrieval", "source_alternative", "random_control")
MAX_REVIEW_BYTES = 64 * 1024 * 1024


def _digest(value: object) -> str:
    return evaluator.digest(evaluator.canonical(value))


def prepare(
    checkout: Path, pack: dict, contexts: dict, pools: list[dict], *, seed: int
) -> tuple[list[dict], dict]:
    """Bind the frozen source and retain every pooled file as unjudged.

    The caller supplies authored task provenance/rubrics and actual pool inputs.
    Pool diversity is checked structurally; this function does not attest that
    the declared retrieval methods were run or that controls are irrelevant.
    """
    require = evaluator.require
    evaluator.object_keys(
        pack,
        [
            "schema_version",
            "suite_id",
            "suite_commitment_sha256",
            "repository_commit",
            "tokenizer",
            "tokenizer_budget_version",
            "routes",
            "file_universe",
            "tasks",
            "comparison_contract",
            "file_universe_digest",
        ],
        "review query pack",
    )
    require(type(seed) is int, "review seed must be an integer")
    require(
        type(pack["schema_version"]) is int and pack["schema_version"] == evaluator.SCHEMA_VERSION,
        "review requires the existing current query pack",
    )
    evaluator.sha(pack["suite_commitment_sha256"], "suite commitment")
    evaluator.string(pack["suite_id"], "suite ID")
    require(
        pack["tokenizer"] == evaluator.TOKENIZER
        and pack["tokenizer_budget_version"] == evaluator.TOKENIZER_BUDGET_VERSION,
        "review query pack tokenizer mismatch",
    )
    evaluator.validate_comparison_contract(
        pack["comparison_contract"], "review comparison contract"
    )
    require(
        isinstance(pack["routes"], list)
        and bool(pack["routes"])
        and all(isinstance(route, str) and route.strip() for route in pack["routes"])
        and len(set(pack["routes"])) == len(pack["routes"]),
        "review routes malformed",
    )
    source = evaluator.SourceSnapshot(
        checkout, pack["repository_commit"], max_total_bytes=source_oracle.MAX_SOURCE_BYTES
    )
    universe, ordered = evaluator.validate_file_universe(source, pack["file_universe"])
    require(
        pack["file_universe_digest"] == evaluator.universe_digest(ordered),
        "review file universe digest mismatch",
    )
    tasks = pack["tasks"]
    require(
        isinstance(tasks, list) and 0 < len(tasks) <= source_oracle.MAX_QUERIES,
        "review tasks empty or over limit",
    )
    task_ids = []
    for task in tasks:
        evaluator.object_keys(task, ["task_id", "query", "query_sha256"], "review task")
        task_ids.append(evaluator.string(task["task_id"], "review task ID"))
        query = evaluator.string(task["query"], "review query")
        require(
            task["query_sha256"] == evaluator.digest(query.encode()), "review query hash mismatch"
        )
    require(len(set(task_ids)) == len(task_ids), "duplicate review task")
    require(
        isinstance(contexts, dict) and set(contexts) == set(task_ids),
        "review context task coverage mismatch",
    )
    for context in contexts.values():
        evaluator.object_keys(context, ["intent", "provenance", "rubric"], "review context")
        for field, value in context.items():
            evaluator.string(value, "review context " + field)
    require(isinstance(pools, list) and bool(pools), "review candidate pools missing")
    pool_ids = set()
    candidates = {task_id: {} for task_id in task_ids}
    coverage = {task_id: {kind: set() for kind in POOL_KINDS} for task_id in task_ids}
    for pool in pools:
        evaluator.object_keys(pool, ["pool_id", "kind", "tasks"], "review pool")
        pool_id = evaluator.string(pool["pool_id"], "review pool ID")
        require(pool_id not in pool_ids, "duplicate review pool ID")
        pool_ids.add(pool_id)
        require(pool["kind"] in POOL_KINDS, "unknown review pool kind")
        require(
            isinstance(pool["tasks"], dict) and set(pool["tasks"]) == set(task_ids),
            "review pool task coverage mismatch",
        )
        for task_id, rows in pool["tasks"].items():
            require(isinstance(rows, list), "review pool candidates malformed")
            coverage[task_id][pool["kind"]].add(pool_id)
            for row in rows:
                evaluator.object_keys(row, ["path", "file_sha256"], "review pooled file")
                path = evaluator.string(row["path"], "review pooled path")
                require(
                    path in universe and row["file_sha256"] == universe[path],
                    "review pooled file source hash/universe mismatch",
                )
                candidates[task_id].setdefault(path, set()).add(pool_id)
    for task_id, kinds in coverage.items():
        require(
            len(kinds["retrieval"]) >= 2
            and kinds["source_alternative"]
            and kinds["random_control"],
            "review requires varied retrieval, source and control pools",
        )
        require(candidates[task_id], "review task has no pooled files")
        for kind in ("source_alternative", "random_control"):
            require(
                any(members & kinds[kind] for members in candidates[task_id].values()),
                "review task lacks source alternatives or random controls",
            )
    # Export complete frozen file text; never silently truncate review evidence.
    paths = sorted({path for rows in candidates.values() for path in rows})
    texts, total_bytes = {}, 0
    for path in paths:
        raw = source.file(path)[0]
        total_bytes += 2 * len(raw) * sum(path in rows for rows in candidates.values())
        require(total_bytes <= MAX_REVIEW_BYTES, "review source export byte limit exceeded")
        try:
            texts[path] = raw.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise evaluator.EvidenceError("review source must be UTF-8: " + path) from exc
    forms = []
    for slot in range(2):
        reviews = []
        for task in tasks:
            task_id = task["task_id"]
            order = sorted(
                candidates[task_id],
                key=lambda path: hashlib.sha256(
                    evaluator.canonical([seed, slot, task_id, path])
                ).digest(),
            )
            reviews.append(
                {
                    **task,
                    **contexts[task_id],
                    "answerable": None,
                    "rationale": "",
                    "files": [
                        {
                            "path": path,
                            "file_sha256": universe[path],
                            "source_text": texts[path],
                            "grade": None,
                            "rationale": "",
                        }
                        for path in order
                    ],
                }
            )
        forms.append(
            {
                "schema_version": 1,
                "status": "unjudged_preparation",
                "form_slot": slot + 1,
                "reviewer_id": None,
                "repository_commit": pack["repository_commit"],
                "file_universe_digest": pack["file_universe_digest"],
                "query_pack_sha256": _digest(pack),
                "reviews": reviews,
            }
        )
    custody = {
        "schema_version": 1,
        "status": "unjudged_preparation",
        "qualified": False,
        "pool_execution_attested": False,
        "query_pack_sha256": _digest(pack),
        "contexts_sha256": _digest(contexts),
        "pools_sha256": _digest(pools),
        "seed": seed,
        "pool_kinds": {pool["pool_id"]: pool["kind"] for pool in pools},
        "membership": {
            task_id: {path: sorted(members) for path, members in sorted(rows.items())}
            for task_id, rows in candidates.items()
        },
        "form_sha256": [_digest(form) for form in forms],
        "receipt_contract": "run._validate_gold_review_receipt",
    }
    return forms, custody


def write(
    checkout: Path, pack: dict, contexts: dict, pools: list[dict], output: Path, *, seed: int
) -> dict:
    """Write unjudged forms to a fresh external root; custody is owner-only."""
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("review output root must be fresh and absolute")
    target = output.resolve()
    for root in (checkout.resolve(), Path(__file__).resolve().parents[3]):
        if target.is_relative_to(root) or root.is_relative_to(target):
            raise ValueError("review output root must be external and disjoint")
    forms, custody = prepare(checkout, pack, contexts, pools, seed=seed)
    output.mkdir(parents=True)
    try:
        for index, form in enumerate(forms, 1):
            (output / f"reviewer-{index}.json").write_bytes(evaluator.canonical(form))
        owner = output / "owner"
        owner.mkdir(mode=0o700)
        (owner / "custody.json").write_bytes(evaluator.canonical(custody))
    except BaseException:
        shutil.rmtree(output)
        raise
    return custody
