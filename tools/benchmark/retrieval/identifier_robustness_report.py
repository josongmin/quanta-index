#!/usr/bin/env python3
"""Join frozen robustness strata with an existing, validated file diagnostic.

This is a diagnostic presentation layer. It never scores candidates and never
promotes different query policies to an equivalent product comparison.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from pathlib import Path
from typing import Any

from tools.benchmark.retrieval import evaluator, query_plan, source_oracle


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _unique(rows: list[dict], where: str) -> dict[str, dict]:
    keys = [row["task_id"] for row in rows]
    _require(len(keys) == len(set(keys)), f"duplicate task ID in {where}")
    return dict(zip(keys, rows))


def _policy(capture: dict) -> dict[str, str | None]:
    system = capture["system"]
    profile = capture["execution_profile"]
    if system == "quanta":
        name = profile["policy"]
        _require(name in query_plan.FILE_PROJECTION_ORDERING, "unsupported Quanta file policy")
        return {
            "policy": name,
            "case": "sensitive" if name != "literal_file" else "normalizer_defined",
            "scope": (
                "content_and_path"
                if name == "keyword_file"
                else "content"
                if name == "substring_file"
                else "policy_unspecified"
            ),
            "ordering": query_plan.FILE_PROJECTION_ORDERING[name],
        }
    if system == "semble":
        _require(profile.get("mode") == "lexical-file", "Semble capture is not file mode")
        return {
            "policy": "semble:lexical-file",
            "case": "product_native_unspecified",
            "scope": "product_native_unspecified",
            "ordering": "score_desc_native_tiebreak",
        }
    raise ValueError(f"unsupported file diagnostic capture system: {system}")


def compose(
    suite: dict[str, Any],
    census: dict[str, Any],
    record: dict[str, Any],
    diagnostic: dict[str, Any],
    lane: str,
) -> dict[str, Any]:
    """Return a source-bound breakdown using evaluator-supplied hit values only."""
    _require(
        diagnostic.get("report_scope") == "single_route_independent_judgment_diagnostic_v1",
        "wrong diagnostic scope",
    )
    _require(diagnostic.get("status") == "diagnostic_unqualified", "unexpected qualification")
    _require(
        diagnostic.get("suite_commitment_sha256") == evaluator.digest(evaluator.canonical(suite)),
        "suite/report mismatch",
    )
    _require(
        diagnostic.get("runner_record_sha256") == evaluator.digest(evaluator.canonical(record)),
        "record/report mismatch",
    )
    _require(diagnostic.get("suite_id") == suite.get("suite_id"), "suite ID mismatch")
    _require(
        diagnostic.get("repository_commit") == suite.get("repository_commit"), "source mismatch"
    )
    _require(
        diagnostic.get("comparison_contract")
        == suite.get("comparison_contract")
        == record.get("comparison_contract"),
        "comparison contract mismatch",
    )
    route = diagnostic["route"]
    _require(suite.get("routes") == [route], "expected one matching route")
    _require(
        diagnostic.get("route_provenance") == record.get("route_provenance"),
        "route provenance mismatch",
    )
    _require(diagnostic.get("captures") == record.get("captures"), "capture mismatch")
    capture = record["captures"][record["route_provenance"][route]["capture_id"]]
    policy = _policy(capture)

    lane_data = census["lanes"][lane]
    tasks = _unique([t for t in suite["tasks"] if t["split"] == "eval"], "suite")
    _require(len(tasks) == len(suite["tasks"]), "non-eval task in diagnostic suite")
    _require(diagnostic.get("selected_eval_tasks") == len(tasks), "selected task count mismatch")
    results = _unique(record["results"], "record")
    _require(set(results) == set(tasks), "missing or extra record task")
    _require(all(row["route"] == route for row in results.values()), "record route mismatch")
    _require(
        all(row.get("rank_unit") == "distinct_file" for row in results.values()),
        "record rank unit mismatch",
    )
    _require(
        all(row.get("ordering") == policy["ordering"] for row in results.values()),
        "record ordering mismatch",
    )

    records = _unique(lane_data["records"], "census")
    if lane == "no-answer-content":
        _require(len(records) == lane_data["admitted"], "content no-answer admission mismatch")
        _require(set(records) == set(tasks), "content no-answer census/suite mismatch")
        source_records = _unique(census["lanes"]["no-answer"]["records"], "source census")
        source_ids = [row["source_task_id"] for row in records.values()]
        _require(len(source_ids) == len(set(source_ids)), "duplicate content no-answer source ID")
        _require(set(source_ids) <= set(source_records), "unknown content no-answer source ID")
        excluded = lane_data["excluded_probes"]
        excluded_ids = [row["source_task_id"] for row in excluded]
        _require(
            len(excluded_ids) == len(set(excluded_ids)), "duplicate content no-answer exclusion"
        )
        _require(
            set(excluded_ids) == set(source_records) - set(source_ids)
            and lane_data["excluded"] == len(excluded),
            "content no-answer exclusion census mismatch",
        )
        for task_id, row in records.items():
            source = source_records.get(row["source_task_id"])
            _require(
                source is not None and source["query"] == tasks[task_id]["query"],
                "content no-answer source mismatch",
            )
            _require(not tasks[task_id]["answerable"], "content no-answer task has gold")
        requested = lane_data["source_admitted"]
        _require(requested == len(source_records), "content no-answer requested mismatch")
        not_admitted = requested - len(tasks)
        strata = {task_id: "no_answer" for task_id in tasks}
        contract = lane_data["contract"]
    else:
        admitted = {task_id: row for task_id, row in records.items() if row["status"] == "admitted"}
        _require(set(tasks) <= set(admitted), "suite task absent from admitted census")
        _require(lane_data["admitted"] == len(admitted), "census admitted count mismatch")
        unsupported = set(admitted) - set(tasks)
        for task_id in unsupported:
            _require(capture["system"] == "quanta", "missing submitted task without policy proof")
            try:
                query_plan.plan_lexical_request(policy["policy"], admitted[task_id]["query"])
            except query_plan.QueryPlanError:
                pass
            else:
                raise ValueError("admitted task missing despite supported query form")
        for task_id, row in admitted.items():
            if task_id not in tasks:
                continue
            task = tasks[task_id]
            _require(
                row["query"] == task["query"] and row["query_family_id"] == task["query_family_id"],
                "census query/family mismatch",
            )
            expected_class = (
                "no_answer"
                if row["matched_names"] == 0
                else "unique"
                if row["matched_names"] == 1
                else "ambiguous"
            )
            _require(row["answer_class"] == expected_class, "answer class mismatch")
            _require(
                (row["answer_class"] == "no_answer") == (not task["answerable"]),
                "answer class mismatch",
            )
            _require(row["gold_files"] == len(task["file_judgments"]), "census gold count mismatch")
        requested = len(records)
        not_admitted = requested - len(admitted)
        strata = {
            task_id: row["answer_class"] for task_id, row in admitted.items() if task_id in tasks
        }
        contract = lane_data["contract"]
    _require(not_admitted >= 0, "negative admission gap")
    _require(
        all(
            t["source_oracle"] == {"contract": contract, "unit": "distinct_file"}
            for t in tasks.values()
        ),
        "source oracle mismatch",
    )

    judgments = diagnostic["judgment_metrics"]["file_judgments"]
    route_summary = judgments["routes"][route]
    _require(route_summary["rank_unit"] == "distinct_file", "diagnostic rank unit mismatch")
    _require(route_summary["ordering"] == policy["ordering"], "diagnostic ordering mismatch")
    answerable = {task_id for task_id, task in tasks.items() if task["answerable"]}
    scored_rows = _unique(judgments["per_query"], "diagnostic per-query")
    _require(set(scored_rows) == answerable, "missing or extra diagnostic task")
    _require(
        all(row["route"] == route for row in scored_rows.values()), "diagnostic route mismatch"
    )
    eligible = {task_id for task_id, row in scored_rows.items() if row["eligible"]}
    _require(eligible == set(route_summary["eligible_task_ids"]), "eligible ID mismatch")
    _require(route_summary["eligible_count"] == len(eligible), "eligible count mismatch")
    _require(
        route_summary["selected_answerable_tasks"] == len(answerable), "answerable count mismatch"
    )
    _require(
        {r["task_id"]: r["reason"] for r in route_summary["excluded"]}
        == {task_id: r["reason"] for task_id, r in scored_rows.items() if not r["eligible"]},
        "excluded reason mismatch",
    )
    statuses = Counter(row["status"] for row in results.values())
    _require(set(statuses) <= set(evaluator.RESULT_STATUSES), "unknown execution status")
    _require(
        dict(Counter(results[task_id]["status"] for task_id in answerable))
        == route_summary["status_counts"],
        "diagnostic status counts mismatch",
    )
    _require(
        sum(diagnostic["no_answer"]["status_counts"].values()) == len(tasks) - len(answerable),
        "no-answer status count mismatch",
    )
    _require(
        dict(Counter(results[task_id]["status"] for task_id in tasks if task_id not in answerable))
        == diagnostic["no_answer"]["status_counts"],
        "no-answer status mismatch",
    )
    no_answer_ids = set(tasks) - answerable
    no_answer_report = diagnostic["no_answer"]
    _require(set(no_answer_report["task_ids"]) == no_answer_ids, "no-answer task ID mismatch")
    _require(no_answer_report["sample_count"] == len(no_answer_ids), "no-answer count mismatch")
    abstained = sum(results[task_id]["status"] == "abstained" for task_id in no_answer_ids)
    _require(no_answer_report["abstained"] == abstained, "no-answer abstained count mismatch")
    _require(
        no_answer_report["abstention_rate"]
        == (abstained / len(no_answer_ids) if no_answer_ids else evaluator.NOT_APPLICABLE),
        "no-answer abstention rate mismatch",
    )
    nonempty_no_answer = sum(bool(results[task_id]["candidates"]) for task_id in no_answer_ids)

    breakdown: dict[str, dict[str, int | None]] = {}
    for label in ("unique", "ambiguous", "no_answer"):
        admitted_ids = {task_id for task_id, value in strata.items() if value == label}
        eligible_ids = admitted_ids & eligible
        hits = 0
        for task_id in eligible_ids:
            score = scored_rows[task_id]["scores"]["hit_at_10"]
            _require(score in (0.0, 1.0), "invalid evaluator hit value")
            hits += int(score)
        breakdown[label] = {
            "admitted": len(admitted_ids),
            "eligible": len(eligible_ids),
            "hit_at_10_count": hits if label != "no_answer" else None,
        }
    _require(
        sum(int(row["admitted"]) for row in breakdown.values()) == len(tasks),
        "stratum sum mismatch",
    )
    _require(
        sum(int(row["eligible"]) for row in breakdown.values()) == len(eligible),
        "eligible stratum sum mismatch",
    )
    return {
        "status": "diagnostic_unqualified",
        "lane": lane,
        "suite_id": suite["suite_id"],
        "suite_commitment_sha256": diagnostic["suite_commitment_sha256"],
        "runner_record_sha256": diagnostic["runner_record_sha256"],
        "repository_commit": suite["repository_commit"],
        "route": route,
        "system": capture["system"],
        "source_oracle_contract": contract,
        # Only the CLI may promote this flag after load_evidence has replayed
        # the source oracle. compose() also serves unit fixtures without source.
        "content_absence_replay_verified": False,
        "rank_unit": "distinct_file",
        **policy,
        "requested": requested,
        "not_admitted": not_admitted,
        "unsupported_query_form": len(unsupported) if lane != "no-answer-content" else 0,
        "submitted": len(tasks),
        "eligible": len(eligible),
        "status_counts": dict(sorted(statuses.items())),
        "no_answer": {
            "sample_count": no_answer_report["sample_count"],
            "abstained": no_answer_report["abstained"],
            "abstention_rate": no_answer_report["abstention_rate"],
            "nonempty_results": nonempty_no_answer,
            "status_counts": no_answer_report["status_counts"],
        },
        "strata": breakdown,
    }


def compose_validated(
    suite: dict[str, Any],
    pack: dict[str, Any],
    record: dict[str, Any],
    diagnostic: dict[str, Any],
    census: dict[str, Any],
    lane: str,
) -> dict[str, Any]:
    """Verify the hit/status subset against evaluator replay before joining.

    Historical diagnostics can differ in optional fields and insignificant
    floating-point NDCG digits. This report consumes only hit, eligibility,
    exclusion, and status, so require exact equality for those fields.
    """
    expected = evaluator.evaluate_diagnostic(suite, pack, record)
    old = diagnostic["judgment_metrics"]["file_judgments"]
    new = expected["judgment_metrics"]["file_judgments"]
    route = diagnostic["route"]

    def consumed_rows(rows: list[dict]) -> dict[str, tuple]:
        return {
            row["task_id"]: (
                row["route"],
                row["eligible"],
                row.get("reason"),
                row.get("scores", {}).get("hit_at_10"),
            )
            for row in rows
        }

    _require(
        consumed_rows(old["per_query"]) == consumed_rows(new["per_query"])
        and old["routes"][route]["status_counts"] == new["routes"][route]["status_counts"]
        and diagnostic["no_answer"]["status_counts"] == expected["no_answer"]["status_counts"],
        "diagnostic differs from evaluator replay",
    )
    return compose(suite, census, record, diagnostic, lane)


def verify_census_against_source(
    repo: Path, suite: dict[str, Any], census: dict[str, Any], lane: str
) -> None:
    """Independently rederive the ambiguity strata from the frozen Go files."""
    if lane == "no-answer-content":
        # New NOC tasks are checked for content absence by validate_suite;
        # legacy NOC is explicitly marked unverified in compose's output.
        return
    source = evaluator.SourceSnapshot(repo, suite["repository_commit"])
    files = {
        item["path"]: (source.file(item["path"])[0], item["file_sha256"])
        for item in suite["file_universe"]
    }
    lane_data = census["lanes"][lane]
    contract = lane_data["contract"]
    _require(
        contract in source_oracle.GO_NAME_CONTRACTS,
        "census contract is not a Go declaration name contract",
    )
    names = {row["query"] for row in lane_data["records"] if row["status"] == "admitted"}
    oracle = source_oracle.SourceOracleIndex(files, names)
    for row in lane_data["records"]:
        if row["status"] != "admitted":
            continue
        matched = oracle.matched_names(contract, row["query"])
        expected_class = (
            "no_answer" if not matched else "unique" if len(matched) == 1 else "ambiguous"
        )
        _require(
            row["matched_names"] == len(matched)
            and row["answer_class"] == expected_class
            and row["gold_files"]
            == len(oracle.expected_rows(contract, row["query"], "distinct_file")),
            "census/source declaration mismatch",
        )


def _write_output(path: Path, rendered: str, *, repo: Path | None = None) -> None:
    _require(path.is_absolute(), "output path must be absolute")
    checkout = Path(__file__).resolve().parents[3]
    _require(
        not path.resolve().is_relative_to(checkout),
        "output path must be outside source checkout",
    )
    if repo is not None:
        _require(
            not path.resolve().is_relative_to(repo.resolve()),
            "output path must be outside corpus checkout",
        )
    with path.open("xb") as stream:
        stream.write(rendered.encode("utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for name in ("repo", "suite", "record", "diagnostic", "census", "lane"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        suite, pack, record = evaluator.load_evidence(
            Path(args.repo).resolve(), Path(args.suite), Path(args.record)
        )
        census = json.loads(Path(args.census).read_text(encoding="utf-8"))
        diagnostic = json.loads(Path(args.diagnostic).read_text(encoding="utf-8"))
        verify_census_against_source(Path(args.repo).resolve(), suite, census, args.lane)
        output = compose_validated(suite, pack, record, diagnostic, census, args.lane)
        output["content_absence_replay_verified"] = (
            args.lane == "no-answer-content"
            and output["source_oracle_contract"] == source_oracle.ASCII_CONTENT_ABSENT_CASEFOLD
        )
        rendered = json.dumps(output, indent=2, sort_keys=True, allow_nan=False) + "\n"
        if args.output:
            _write_output(args.output, rendered, repo=Path(args.repo))
        else:
            sys.stdout.write(rendered)
        return 0
    except (ValueError, KeyError, TypeError, OSError, evaluator.EvidenceError) as exc:
        print("ERROR: " + str(exc), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
