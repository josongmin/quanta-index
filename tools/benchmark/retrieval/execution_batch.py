"""Bind separately validated blind packs to one product execution pack.

This is an execution projection, not an evaluator suite. Near-duplicate and
request-mode rules remain attached to each original suite. A product receives
only the union of its blind queries; the membership map names any query shared
by multiple suites without inventing another product response.
"""

from __future__ import annotations

import copy
import hashlib
from collections.abc import Callable, Iterator, Sequence
from pathlib import Path
from typing import Any

try:
    from tools.benchmark.evidence import _read_control_file, parse_json
    from tools.benchmark.retrieval.evaluator import canonical, digest
except ImportError:  # direct run.py invocation from an external working directory
    from evaluator import canonical, digest
    from evidence import _read_control_file, parse_json


class BatchError(ValueError):
    """An execution batch cannot preserve its member-pack contract."""


SHARED_FIELDS = (
    "schema_version",
    "repository_commit",
    "tokenizer",
    "tokenizer_budget_version",
    "routes",
    "file_universe",
    "file_universe_digest",
    "comparison_contract",
)
PACK_FIELDS = set(SHARED_FIELDS) | {"suite_id", "suite_commitment_sha256", "tasks"}
TASK_FIELDS = {"task_id", "query", "query_sha256"}


def iter_repository_admissions(
    root: Path,
    repositories: Sequence[str],
    *,
    upstream_alive: Callable[[], bool],
    wait: Callable[[], None],
    repository_failure: Callable[[str], str | None] | None = None,
    repository_cells: dict[str, Path] | None = None,
) -> Iterator[tuple[str, dict]]:
    """Drain every ready/failed repository before waiting for pending admissions.

    Readiness is not admission validation: the consumer must verify the returned
    authority and its source/input bindings before execution. A failed or missing
    admission is always a failed outcome, never a successful empty product cell.
    The caller owns its poll/deadline policy and durable per-cell result writing.
    """
    if isinstance(repositories, (str, bytes)):
        raise BatchError("invalid admission repository list")
    pending = list(repositories)
    if (
        not pending
        or any(
            not isinstance(repo, str) or not repo or Path(repo).name != repo or repo in {".", ".."}
            for repo in pending
        )
        or len(set(pending)) != len(pending)
    ):
        raise BatchError("invalid admission repository list")
    if repository_cells is not None and (
        set(repository_cells) != set(pending)
        or any(
            not isinstance(path, Path) or not path.is_absolute()
            for path in repository_cells.values()
        )
        or len({path.resolve() for path in repository_cells.values()}) != len(repository_cells)
    ):
        raise BatchError("invalid admission repository cell mapping")

    def cell_for(repo: str) -> Path:
        return repository_cells[repo] if repository_cells is not None else root / repo

    while pending:
        for repo in pending[:]:
            cell = cell_for(repo)
            result, failure = cell / "result.json", cell / "failure.json"
            if result.exists() and failure.exists():
                outcome = {"status": "FAILED", "reason": "conflicting admission terminals"}
            elif failure.exists():
                outcome = {"status": "FAILED", "reason": "repository admission failure terminal"}
            elif result.exists():
                try:
                    outcome = parse_json(_read_control_file(result).decode("utf-8"))
                    if not isinstance(outcome, dict) or outcome.get("status") != "VERIFIED":
                        raise ValueError("admission result is not VERIFIED")
                except (OSError, ValueError, UnicodeDecodeError) as error:
                    outcome = {
                        "status": "FAILED",
                        "reason": "invalid admission result: " + str(error),
                    }
            else:
                reason = repository_failure(repo) if repository_failure is not None else None
                if reason is None:
                    continue
                if not isinstance(reason, str) or not reason:
                    raise BatchError("invalid repository failure reason")
                outcome = {"status": "FAILED", "reason": reason}
            pending.remove(repo)
            yield repo, outcome
        if pending:
            if (root / "pipeline-terminal.json").exists() or not upstream_alive():
                # Recheck once after observing termination, so a just-published
                # successful admission is not misclassified as missing.
                for repo in pending[:]:
                    cell = cell_for(repo)
                    if (cell / "result.json").exists() or (cell / "failure.json").exists():
                        break
                else:
                    for repo in pending:
                        yield (
                            repo,
                            {"status": "FAILED", "reason": "upstream ended without admission"},
                        )
                    return
                continue
            wait()


def build_execution_pack(packs: Sequence[dict[str, Any]]) -> tuple[dict, dict]:
    """Return a native product pack and a deterministic, auditable membership map.

    Callers independently validate every original suite and compare each blind
    pack to the suite-derived pack before this function. This function enforces
    the cross-pack invariants and never reads gold.
    """
    if len(packs) < 2:
        raise BatchError("execution batch requires at least two member packs")
    if any(not isinstance(pack, dict) or set(pack) != PACK_FIELDS for pack in packs):
        raise BatchError("execution batch member pack has unknown or missing keys")
    ordered = sorted(packs, key=lambda pack: str(pack["suite_id"]))
    suite_ids = [pack["suite_id"] for pack in ordered]
    if any(not isinstance(value, str) or not value for value in suite_ids):
        raise BatchError("execution batch member suite ID is invalid")
    if len(set(suite_ids)) != len(suite_ids):
        raise BatchError("execution batch member suite IDs must be unique")
    first = ordered[0]
    if first["schema_version"] != 3:
        raise BatchError("execution batch requires v3 blind packs")
    for pack in ordered[1:]:
        for field in SHARED_FIELDS:
            if pack[field] != first[field]:
                raise BatchError(f"execution batch member {pack['suite_id']} differs at {field}")

    execution_tasks: list[dict] = []
    members: list[dict] = []
    seen_task_ids: set[str] = set()
    by_query_hash: dict[str, tuple[str, str]] = {}
    for pack in ordered:
        tasks = pack["tasks"]
        if not isinstance(tasks, list) or not tasks:
            raise BatchError("execution batch member has no tasks")
        if (
            not isinstance(pack["suite_commitment_sha256"], str)
            or len(pack["suite_commitment_sha256"]) != 64
        ):
            raise BatchError("execution batch member suite commitment is invalid")
        mapped_tasks: list[dict] = []
        for task in tasks:
            if not isinstance(task, dict) or set(task) != TASK_FIELDS:
                raise BatchError("execution batch task has unknown or missing keys")
            task_id, query, query_sha = (task[key] for key in ("task_id", "query", "query_sha256"))
            if (
                not isinstance(task_id, str)
                or not task_id
                or not isinstance(query, str)
                or not query
                or not isinstance(query_sha, str)
                or hashlib.sha256(query.encode("utf-8")).hexdigest() != query_sha
            ):
                raise BatchError("execution batch task identity is invalid")
            if task_id in seen_task_ids:
                raise BatchError(f"execution batch repeats task ID: {task_id}")
            seen_task_ids.add(task_id)
            prior = by_query_hash.get(query_sha)
            if prior is None:
                by_query_hash[query_sha] = (query, task_id)
                execution_tasks.append(dict(task))
                execution_task_id = task_id
            else:
                if prior[0] != query:
                    raise BatchError("execution batch query digest collision")
                execution_task_id = prior[1]
            mapped_tasks.append(
                {
                    "task_id": task_id,
                    "query_sha256": query_sha,
                    "execution_task_id": execution_task_id,
                }
            )
        members.append(
            {
                "suite_id": pack["suite_id"],
                "suite_commitment_sha256": pack["suite_commitment_sha256"],
                "blind_pack_sha256": digest(canonical(pack)),
                "tasks": mapped_tasks,
            }
        )

    batch_commitment = digest(
        canonical({"kind": "retrieval_execution_batch_v1", "members": members})
    )
    execution_pack = {
        **{field: first[field] for field in SHARED_FIELDS},
        "suite_id": f"execution-batch-{batch_commitment[:16]}",
        "suite_commitment_sha256": batch_commitment,
        "tasks": execution_tasks,
    }
    membership = {
        "schema_version": 1,
        "kind": "retrieval_execution_batch_v1",
        "execution_pack_sha256": digest(canonical(execution_pack)),
        "members": members,
    }
    return execution_pack, membership


def verify_execution_membership(
    packs: Sequence[dict[str, Any]], execution_pack: dict, membership: dict
) -> None:
    """Re-derive both artifacts from original pack bytes; do not trust a map alone."""
    expected_pack, expected_membership = build_execution_pack(packs)
    if execution_pack != expected_pack or membership != expected_membership:
        raise BatchError("execution pack or membership differs from original blind packs")


def execution_validation_view(execution_pack: dict) -> dict:
    """Minimal gold-free context for the existing record/source validator.

    This is deliberately not an evaluator suite and must never be scored.
    Original suites remain the only authority for relevance judgments.
    """
    return {
        "schema_version": execution_pack["schema_version"],
        "suite_id": execution_pack["suite_id"],
        "repository_commit": execution_pack["repository_commit"],
        "comparison_contract": copy.deepcopy(execution_pack["comparison_contract"]),
        "routes": list(execution_pack["routes"]),
        "file_universe": copy.deepcopy(execution_pack["file_universe"]),
        "file_universe_digest": execution_pack["file_universe_digest"],
        "tasks": [
            {"task_id": task["task_id"], "query": task["query"], "split": "eval"}
            for task in execution_pack["tasks"]
        ],
    }


def project_scoring_view(
    execution_pack: dict, membership: dict, member_pack: dict, execution_record: dict
) -> dict:
    """Map one validated union record to original task IDs for scoring only.

    The returned object is derived data, not a native product record. The
    caller must first validate the full execution record against the frozen
    source, then validate this view against the original member suite/pack.
    """
    if membership.get("execution_pack_sha256") != digest(canonical(execution_pack)):
        raise BatchError("execution membership pack digest changed")
    if execution_record.get("query_pack_sha256") != membership["execution_pack_sha256"]:
        raise BatchError("execution record does not name the batch pack")
    member_hash = digest(canonical(member_pack))
    matched = [
        member
        for member in membership.get("members", [])
        if member.get("blind_pack_sha256") == member_hash
        and member.get("suite_id") == member_pack.get("suite_id")
    ]
    if len(matched) != 1:
        raise BatchError("member pack is absent or ambiguous in execution membership")
    member = matched[0]
    routes = execution_pack["routes"]
    if member_pack.get("routes") != routes:
        raise BatchError("member routes differ from the full execution pack")
    rows = execution_record.get("results")
    if not isinstance(rows, list):
        raise BatchError("execution record results are missing")
    by_key = {(row.get("task_id"), row.get("route")): row for row in rows if isinstance(row, dict)}
    expected = {(task["task_id"], route) for task in execution_pack["tasks"] for route in routes}
    if len(by_key) != len(rows) or set(by_key) != expected:
        raise BatchError("execution record task/route set is incomplete or duplicated")
    mapped = member["tasks"]
    if [task["task_id"] for task in mapped] != [task["task_id"] for task in member_pack["tasks"]]:
        raise BatchError("member task order differs from execution membership")
    execution_queries = {task["task_id"]: task["query_sha256"] for task in execution_pack["tasks"]}
    for mapped_task, original_task in zip(mapped, member_pack["tasks"], strict=True):
        if (
            mapped_task["query_sha256"] != original_task["query_sha256"]
            or execution_queries.get(mapped_task["execution_task_id"])
            != original_task["query_sha256"]
        ):
            raise BatchError("member query does not match its execution task")
    projected = copy.deepcopy(execution_record)
    projected["query_pack_sha256"] = member_hash
    projected["results"] = []
    for task in mapped:
        for route in routes:
            row = copy.deepcopy(by_key[(task["execution_task_id"], route)])
            row["task_id"] = task["task_id"]
            projected["results"].append(row)
    return projected
