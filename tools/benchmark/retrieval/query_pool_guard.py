"""Check unreviewed query proposals against frozen suites before review or search.

This is an early diagnostic gate. Qualified admission still rechecks complete
development/holdout suites and their source-bound gold with experiment custody.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any

from tools.benchmark.retrieval import evaluator

MAX_POOL_BYTES = 4 * 1024 * 1024
MAX_POOL_ROWS = 2_000
MAX_QUERY_BYTES = 4_096
MAX_REPORTED_CONFLICTS = 10_000
PROPOSAL_ID = re.compile(r"[A-Za-z][A-Za-z0-9_.-]*\Z")
UNREVIEWED_PROPOSAL_STATUS = "unreviewed_query_proposal"


def _regular_bytes(path: Path) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise evaluator.EvidenceError(f"query pool input is not a regular file: {path}")
    if path.stat().st_size > MAX_POOL_BYTES:
        raise evaluator.EvidenceError(f"query pool input exceeds {MAX_POOL_BYTES} bytes: {path}")
    raw = path.read_bytes()
    if len(raw) > MAX_POOL_BYTES:
        raise evaluator.EvidenceError(f"query pool input changed beyond byte limit: {path}")
    return raw


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _json(raw: bytes, source: str) -> Any:
    try:
        return json.loads(raw, object_pairs_hook=_unique_object)
    except (ValueError, UnicodeError) as exc:
        raise evaluator.EvidenceError(f"invalid or duplicate JSON: {source}: {exc}") from exc


def _proposals(
    raw: bytes, source: str, *, require_unreviewed: bool = False
) -> list[tuple[str, str]]:
    rows: list[tuple[str, str]] = []
    seen: set[str] = set()
    try:
        lines = raw.decode("utf-8").splitlines()
    except UnicodeError as exc:
        raise evaluator.EvidenceError(f"query pool is not UTF-8: {source}") from exc
    if not 1 <= len(lines) <= MAX_POOL_ROWS:
        raise evaluator.EvidenceError(f"query pool row count is out of bounds: {source}")
    for index, line in enumerate(lines, start=1):
        row = _json(line.encode("utf-8"), f"{source}:{index}")
        if not isinstance(row, dict) or set(row) != {"proposal_id", "query", "stratum", "status"}:
            raise evaluator.EvidenceError(f"proposal fields differ: {source}:{index}")
        task_id, query = row["proposal_id"], row["query"]
        if (
            not isinstance(task_id, str)
            or PROPOSAL_ID.fullmatch(task_id) is None
            or task_id in seen
            or not isinstance(query, str)
            or not query.strip()
            or "\x00" in query
            or len(query.encode("utf-8")) > MAX_QUERY_BYTES
            or not evaluator.normalize_query(query)
            or not isinstance(row["stratum"], str)
            or not row["stratum"].strip()
            or not isinstance(row["status"], str)
            or not row["status"].strip()
            or (require_unreviewed and row["status"] != UNREVIEWED_PROPOSAL_STATUS)
        ):
            raise evaluator.EvidenceError(f"invalid or duplicate proposal: {source}:{index}")
        seen.add(task_id)
        rows.append((task_id, query))
    return rows


def scan(references: list[tuple[str, str]], candidates: list[tuple[str, str]]) -> dict[str, Any]:
    """Report bounded candidate conflicts; reference-reference pairs are historical data."""
    if not references or not candidates:
        raise evaluator.EvidenceError("query pool needs reference and candidate queries")
    if len(references) + len(candidates) > MAX_POOL_ROWS:
        raise evaluator.EvidenceError("combined query pool exceeds task limit")
    prior = [
        (
            task_id,
            evaluator.normalize_query(query),
            evaluator.query_shingles(evaluator.normalize_query(query)),
        )
        for task_id, query in references
    ]
    conflicts: list[dict[str, Any]] = []
    truncated = False
    maximum = 0.0
    for task_id, query in candidates:
        normalized = evaluator.normalize_query(query)
        grams = evaluator.query_shingles(normalized)
        for other_id, other_normalized, other_grams in prior:
            score = evaluator.shingle_jaccard(grams, other_grams)
            maximum = max(maximum, score)
            if normalized == other_normalized or score >= evaluator.QUERY_NEAR_DUP_JACCARD:
                if len(conflicts) < MAX_REPORTED_CONFLICTS:
                    conflicts.append(
                        {
                            "candidate_id": task_id,
                            "reference_id": other_id,
                            "kind": "normalized_duplicate"
                            if normalized == other_normalized
                            else "near_duplicate",
                            "shingle_jaccard": round(score, 6),
                        }
                    )
                else:
                    truncated = True
        prior.append((task_id, normalized, grams))
    return {
        "status": "pass" if not conflicts else "fail",
        "reference_count": len(references),
        "candidate_count": len(candidates),
        "near_duplicate_threshold": evaluator.QUERY_NEAR_DUP_JACCARD,
        "maximum_shingle_jaccard": round(maximum, 6),
        "conflicts": conflicts,
        "conflicts_truncated": truncated,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--reference-suite", type=Path, action="append", required=True)
    parser.add_argument("--reference-proposals", type=Path, action="append", default=[])
    parser.add_argument("--candidate-proposals", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    try:
        references: list[tuple[str, str]] = []
        bindings: dict[str, str] = {}
        for index, path in enumerate(args.reference_suite, start=1):
            suite_raw = _regular_bytes(path)
            suite, _pack, _source = evaluator.validate_suite(
                args.repo.resolve(), _json(suite_raw, str(path))
            )
            references.extend(
                (f"suite{index}/" + task["task_id"], task["query"]) for task in suite["tasks"]
            )
            bindings[f"reference_suite_{index}_sha256"] = hashlib.sha256(suite_raw).hexdigest()
        for index, path in enumerate(args.reference_proposals, start=1):
            raw = _regular_bytes(path)
            references.extend(
                (f"prior{index}/{task_id}", query) for task_id, query in _proposals(raw, str(path))
            )
            bindings[f"reference_proposals_{index}_sha256"] = hashlib.sha256(raw).hexdigest()
        candidate_raw = _regular_bytes(args.candidate_proposals)
        candidates = [
            ("candidate/" + task_id, query)
            for task_id, query in _proposals(
                candidate_raw, str(args.candidate_proposals), require_unreviewed=True
            )
        ]
        bindings["candidate_proposals_sha256"] = hashlib.sha256(candidate_raw).hexdigest()
        report = {**bindings, **scan(references, candidates)}
        rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
        if args.output:
            with args.output.open("x", encoding="utf-8") as output:
                output.write(rendered)
        else:
            sys.stdout.write(rendered)
        return 0 if report["status"] == "pass" else 2
    except (evaluator.EvidenceError, OSError, ValueError) as exc:
        print("ERROR: " + str(exc), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
