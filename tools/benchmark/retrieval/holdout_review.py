"""Prepare blind file-review forms and reissue reviewed NL file diagnostics.

This is a preparation adapter, not a labeler or a qualification pipeline.
Candidates from retrieval, source alternatives and random controls are pooled
by file. Product identities and pool membership stay in the owner-only custody
document; scores and ranks are rejected. Human decisions must subsequently enter the
existing evaluator judgments and run.py annotation/adjudication receipts.
Diagnostic projection preserves supplied labels and review identities without
attesting human provenance or transferring the original admission receipts.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import shutil
from pathlib import Path

from tools.benchmark.evidence import _read_control_file, parse_json
from tools.benchmark.retrieval import evaluator, query_plan, source_oracle

POOL_KINDS = ("retrieval", "source_alternative", "random_control")
MAX_REVIEW_BYTES = 64 * 1024 * 1024


def project_natural_language_file_diagnostic(
    checkout: Path, suite_bytes: bytes, *, suite_id: str
) -> tuple[dict, dict, dict]:
    """Reissue reviewed NL tasks without carrying mixed-suite admission claims.

    The complete input suite must still validate. Every selected query, label,
    family and review identity is preserved; only its request contract changes.
    Review identities remain self-reported and existing receipts still bind the
    original suite, so this projection cannot qualify a comparison.
    """
    original = parse_json(suite_bytes.decode("utf-8"))
    evaluator.validate_suite(checkout, original)
    evaluator.string(suite_id, "projected suite ID")
    evaluator.require(suite_id != original["suite_id"], "projection requires a new suite ID")
    selected = [task for task in original["tasks"] if task.get("query_intent") == "semantic_intent"]
    evaluator.require(bool(selected), "reviewed NL projection has no semantic_intent tasks")
    for task in selected:
        evaluator.require(
            task["split"] == "eval"
            and "source_oracle" not in task
            and "file_judgments" in task
            and task.get("judgment_policy") == evaluator.COMPLETE_JUDGMENT_POLICY
            and task.get("label_review", {}).get("assessment")
            in evaluator.LABEL_REVIEW_ASSESSMENTS[1:],
            "NL projection requires reviewed eval file judgments: " + task["task_id"],
        )
        query_plan.plan_lexical_request("natural_language_file", task["query"])
    projected = copy.deepcopy(original)
    projected["suite_id"] = suite_id
    projected["routes"] = ["lexical", "semble-lexical-file"]
    projected["diagnostic_policy"] = evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    projected["tasks"] = copy.deepcopy(selected)
    contract = {
        "request_mode": query_plan.NATURAL_LANGUAGE_FILE_SEARCH,
        "gold_unit": "distinct_file",
        "result_unit": "distinct_file",
    }
    for task in projected["tasks"]:
        task["evaluation_contract"] = dict(contract)
    checked, pack, _source = evaluator.validate_suite(checkout, projected)
    selected_ids = {task["task_id"] for task in selected}
    lineage = {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "qualified": False,
        "human_provenance_attested": False,
        "split_admission": "not_carried_forward",
        "review_receipts": "remain_bound_to_original_suite",
        "input_suite_bytes_sha256": evaluator.digest(suite_bytes),
        "input_suite_canonical_sha256": _digest(original),
        "suite_sha256": _digest(checked),
        "blind_pack_sha256": _digest(pack),
        "repository_commit": checked["repository_commit"],
        "file_universe_digest": checked["file_universe_digest"],
        "preserved_task_fields_except": ["evaluation_contract"],
        "selected_task_ids": [task["task_id"] for task in selected],
        "excluded_task_ids": [
            task["task_id"] for task in original["tasks"] if task["task_id"] not in selected_ids
        ],
    }
    return checked, pack, lineage


def write_natural_language_file_diagnostic(
    checkout: Path, suite_path: Path, output: Path, *, suite_id: str
) -> dict:
    """Write the projection to a fresh external root, rechecking its inputs."""
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("NL diagnostic output root must be fresh and absolute")
    target = output.resolve()
    for root in (
        checkout.resolve(),
        suite_path.parent.resolve(),
        Path(__file__).resolve().parents[3],
    ):
        if target.is_relative_to(root) or root.is_relative_to(target):
            raise ValueError("NL diagnostic output root must be external and disjoint")
    sources = [Path(__file__), Path(evaluator.__file__), Path(query_plan.__file__)]
    source_digests = {path.name: evaluator.digest(path.read_bytes()) for path in sources}
    raw = _read_control_file(suite_path)
    suite, pack, lineage = project_natural_language_file_diagnostic(
        checkout, raw, suite_id=suite_id
    )
    evaluator.validate_file_universe(
        evaluator.SourceSnapshot(checkout, suite["repository_commit"]), suite["file_universe"]
    )
    if raw != _read_control_file(suite_path) or source_digests != {
        path.name: evaluator.digest(path.read_bytes()) for path in sources
    }:
        raise ValueError("NL diagnostic input or tool changed during projection")
    lineage["tool_source_sha256"] = source_digests
    output.mkdir(parents=True)
    try:
        for name, payload in (
            ("suite.json", suite),
            ("blind-pack.json", pack),
            ("lineage.json", lineage),
        ):
            (output / name).write_bytes(evaluator.canonical(payload))
    except BaseException:
        shutil.rmtree(output)
        raise
    return lineage


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


def validate_completed_forms(
    checkout: Path,
    pack: dict,
    contexts: dict,
    pools: list[dict],
    completed: list[dict],
    *,
    seed: int,
) -> dict:
    """Check two completed forms against frozen inputs; expose disagreements.

    Reviewer identities are self-reported. This check cannot establish human
    provenance, complete relevance outside the pool, or adjudicated qrels.
    """
    expected, custody = prepare(checkout, pack, contexts, pools, seed=seed)
    require = evaluator.require
    require(isinstance(completed, list) and len(completed) == 2, "two review forms required")
    reviewer_ids = []
    for form, template in zip(completed, expected, strict=True):
        require(isinstance(form, dict), "review form malformed")
        require(set(form) == set(template), "review form fields changed")
        reviewer_ids.append(evaluator.string(form["reviewer_id"], "reviewer identity"))
        for key in set(template) - {"reviewer_id", "reviews"}:
            require(form[key] == template[key], "review form source binding changed: " + key)
        require(
            isinstance(form["reviews"], list) and len(form["reviews"]) == len(template["reviews"]),
            "review task coverage changed",
        )
        for row, frozen in zip(form["reviews"], template["reviews"], strict=True):
            require(isinstance(row, dict) and set(row) == set(frozen), "review task fields changed")
            for key in set(frozen) - {"answerable", "rationale", "files"}:
                require(row[key] == frozen[key], "review query/context changed: " + key)
            require(type(row["answerable"]) is bool, "review answerability missing")
            evaluator.string(row["rationale"], "review task rationale")
            require(
                isinstance(row["files"], list) and len(row["files"]) == len(frozen["files"]),
                "review candidate coverage changed",
            )
            has_positive = False
            for file_row, source_row in zip(row["files"], frozen["files"], strict=True):
                require(
                    isinstance(file_row, dict) and set(file_row) == set(source_row),
                    "review candidate fields changed",
                )
                for key in set(source_row) - {"grade", "rationale"}:
                    require(
                        file_row[key] == source_row[key], "review source/candidate changed: " + key
                    )
                grade = file_row["grade"]
                require(type(grade) is int and 0 <= grade <= 3, "review grade must be 0..3")
                evaluator.string(file_row["rationale"], "review file rationale")
                has_positive |= grade > 0
            require(
                row["answerable"] or not has_positive,
                "review cannot deny answerability while grading a file relevant",
            )
    require(len(set(reviewer_ids)) == 2, "two distinct reviewer identities required")

    by_slot = [{row["task_id"]: row for row in form["reviews"]} for form in completed]
    disagreements = []
    for task in pack["tasks"]:
        task_id = task["task_id"]
        left, right = (rows[task_id] for rows in by_slot)
        grade_by_slot = [
            {row["path"]: row["grade"] for row in review["files"]} for review in (left, right)
        ]
        file_disagreements = [
            {
                "path": path,
                "file_sha256": file_hash,
                "grades": [grade_by_slot[0][path], grade_by_slot[1][path]],
            }
            for path, file_hash in sorted(
                (row["path"], row["file_sha256"]) for row in left["files"]
            )
            if grade_by_slot[0][path] != grade_by_slot[1][path]
        ]
        if left["answerable"] != right["answerable"] or file_disagreements:
            disagreements.append(
                {
                    "task_id": task_id,
                    "query_sha256": task["query_sha256"],
                    "answerable": [left["answerable"], right["answerable"]],
                    "files": file_disagreements,
                }
            )
    return {
        "schema_version": 1,
        "status": "completed_forms_validated_unqualified",
        "qualified": False,
        "human_provenance_attested": False,
        "pool_execution_attested": False,
        "query_pack_sha256": custody["query_pack_sha256"],
        "template_sha256": custody["form_sha256"],
        "completed_form_sha256": [_digest(form) for form in completed],
        "reviewer_ids": reviewer_ids,
        "task_count": len(pack["tasks"]),
        "disagreements": disagreements,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Project reviewed NL tasks to a file diagnostic")
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--suite", required=True, type=Path)
    parser.add_argument("--suite-id", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    lineage = write_natural_language_file_diagnostic(
        args.repo, args.suite, args.output, suite_id=args.suite_id
    )
    print(f"projected {len(lineage['selected_task_ids'])} diagnostic NL tasks at {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
