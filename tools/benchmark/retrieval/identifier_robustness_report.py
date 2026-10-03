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

from tools.benchmark.retrieval import (
    evaluator,
    identifier_osa1_absence_suite,
    query_plan,
    source_oracle,
    source_oracle_suite,
)

NEGATIVE_LANES = frozenset(
    ("no-answer-content", "typo-content-absence", identifier_osa1_absence_suite.TARGET_LANE)
)


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _unique(rows: list[dict], where: str) -> dict[str, dict]:
    keys = [row["task_id"] for row in rows]
    _require(len(keys) == len(set(keys)), f"duplicate task ID in {where}")
    return dict(zip(keys, rows))


def _lane_data(census: dict[str, Any], lane: str) -> dict[str, Any]:
    """Resolve a mode projection to its single source-backed query census."""
    lanes = census.get("lanes")
    _require(isinstance(lanes, dict) and lane in lanes, "unknown robustness lane")
    selected = lanes[lane]
    _require(isinstance(selected, dict), "malformed robustness lane")
    if not lane.startswith("default-typo-"):
        return selected
    _require(
        set(selected) == {"derived_from", "product_request_mode", "admitted"}
        and selected["product_request_mode"] == "default_file_search"
        and lane == "default-" + selected["derived_from"]
        and selected["derived_from"] in lanes,
        "invalid default robustness projection",
    )
    source = lanes[selected["derived_from"]]
    _require(
        isinstance(source, dict)
        and "derived_from" not in source
        and source.get("gold_kind") == "intended_original_name"
        and selected["admitted"] == source.get("admitted"),
        "default robustness projection source mismatch",
    )
    return source


def _category_breakdown(
    admitted: dict[str, dict],
    scored: dict[str, dict],
    results: dict[str, dict],
    field: str,
) -> dict[str, dict[str, int]]:
    """Count one frozen census stratum without turning capped rows into failures."""
    if not admitted or not all(field in row for row in admitted.values()):
        return {}
    output: dict[str, dict[str, int]] = {}
    for task_id, row in admitted.items():
        value = row[field]
        _require(isinstance(value, str) and bool(value), f"invalid {field} stratum")
        bucket = output.setdefault(
            value, {"admitted": 0, "eligible": 0, "hit_at_10_count": 0, "capped": 0}
        )
        bucket["admitted"] += 1
        if task_id not in scored or not scored[task_id]["eligible"]:
            continue
        score = scored[task_id]["scores"]["hit_at_10"]
        _require(score in (0.0, 1.0), "invalid evaluator hit value")
        bucket["eligible"] += 1
        bucket["hit_at_10_count"] += int(score)
        bucket["capped"] += results[task_id]["status"] == "capped"
    return dict(sorted(output.items()))


def _sampling_breakdown(
    census: dict[str, Any],
    records: dict[str, dict],
    admitted: dict[str, dict],
    scored: dict[str, dict],
    results: dict[str, dict],
) -> dict[str, dict[str, int]]:
    """Keep the seeded sample separate from the deliberately added stress stratum."""
    if "random_sample_family_ids" not in census:
        return {}
    random_ids = census["random_sample_family_ids"]
    full_population = census.get("generation") in {
        "paired_full_osa1_casefold_v2",
        "paired_full_osa1_casefold_v3",
    }
    multi_ids = census.get(
        "multi_file_family_ids" if full_population else "multi_file_stratum_family_ids"
    )
    _require(
        isinstance(random_ids, list)
        and isinstance(multi_ids, list)
        and all(isinstance(value, str) and value for value in random_ids + multi_ids)
        and len(random_ids) == len(set(random_ids))
        and len(multi_ids) == len(set(multi_ids)),
        "invalid sampling family IDs",
    )
    random, multi = set(random_ids), set(multi_ids)
    all_families = {row["query_family_id"] for row in records.values()}
    if full_population:
        _require(
            random | multi <= all_families and census["population_families"] == len(all_families),
            "sampling family census mismatch",
        )
    else:
        _require(
            all_families == random | multi
            and census["random_sample_families"] == len(random)
            and census["multi_file_stratum_families"] == len(multi)
            and census["overlap_random_and_multi_file"] == len(random & multi),
            "sampling family census mismatch",
        )
    annotated = {
        task_id: {
            "sampling": "seeded_random"
            if row["query_family_id"] in random
            else "additional_multi_file"
            if row["query_family_id"] in multi
            else "remaining_population"
        }
        for task_id, row in admitted.items()
    }
    return _category_breakdown(annotated, scored, results, "sampling")


def verify_generation_manifest(
    manifest_path: Path,
    census_path: Path,
    suite_path: Path,
    suite: dict[str, Any],
    census: dict[str, Any],
    lane: str,
) -> str:
    """Bind the admission denominator and submitted suite to one frozen generation."""
    raw = manifest_path.read_bytes()
    manifest = evaluator.read_json(manifest_path)
    _require(isinstance(manifest, dict), "invalid robustness generation manifest")
    _require(
        type(manifest.get("schema_version")) is int and manifest["schema_version"] == 1,
        "unsupported generation manifest",
    )
    _require(
        manifest.get("qualification") == "diagnostic_unqualified_source_exposed",
        "unexpected generation qualification",
    )
    _require(
        manifest.get("repository_commit") == suite["repository_commit"],
        "generation source mismatch",
    )
    artifacts = manifest.get("artifacts")
    _require(isinstance(artifacts, list), "missing generation artifacts")
    paths: dict[str, str] = {}
    for item in artifacts:
        _require(
            isinstance(item, dict) and set(item) == {"path", "sha256"},
            "invalid generation artifact",
        )
        path = item["path"]
        _require(isinstance(path, str) and path not in paths, "duplicate generation artifact")
        paths[path] = evaluator.sha(item["sha256"], "generation artifact sha256")
    if census.get("generation") in {
        "paired_full_osa1_casefold_v2",
        "paired_full_osa1_casefold_v3",
    }:
        _require(
            manifest.get("parameters", {}).get("paired_full") is True
            and manifest.get("parameters", {}).get("seed") == census.get("seed"),
            "paired generation parameters mismatch",
        )
        for name, expected in paths.items():
            artifact = (manifest_path.parent / name).resolve()
            _require(
                artifact.is_relative_to(manifest_path.parent.resolve())
                and artifact.is_file()
                and evaluator.digest(artifact.read_bytes()) == expected,
                "generation artifact mismatch: " + name,
            )
        tool_files = manifest.get("tool_files", [])
        _require(
            isinstance(tool_files, list)
            and all(
                isinstance(item, dict)
                and set(item) == {"path", "sha256"}
                and paths.get("tool-sources/" + item["path"]) == item["sha256"]
                for item in tool_files
            )
            and {item["path"] for item in tool_files}
            == {
                name.removeprefix("tool-sources/")
                for name in paths
                if name.startswith("tool-sources/")
            },
            "generation tool snapshot mismatch",
        )
    for name, path in (("census.json", census_path), (f"{lane}-suite.json", suite_path)):
        _require(name in paths, "missing generation artifact: " + name)
        observed = evaluator.digest(path.read_bytes())
        if name == f"{lane}-suite.json" and observed != paths[name]:
            generated_path = manifest_path.parent / name
            generated = evaluator.read_json(generated_path)
            if evaluator.digest(generated_path.read_bytes()) == paths[name] and generated == suite:
                # The runner may reserialize an unchanged suite canonically.
                observed = paths[name]
            # The pair runner adds exactly one external route. Bind all other
            # bytes back to the generator's single-route suite. A Quanta-only
            # capture of a generated paired clean suite removes that route.
            elif suite.get("routes") == ["lexical", "semble-lexical-file"]:
                source_suite = {**suite, "routes": ["lexical"]}
                observed = evaluator.digest(source_oracle_suite._json_bytes(source_suite))
            elif suite.get("routes") == ["lexical"]:
                if (
                    evaluator.digest(generated_path.read_bytes()) == paths[name]
                    and isinstance(generated, dict)
                    and generated.get("routes") == ["lexical", "semble-lexical-file"]
                    and {**generated, "routes": ["lexical"]} == suite
                ):
                    observed = paths[name]
            else:
                raise ValueError("unsupported robustness pair route projection")
        _require(
            observed == paths[name],
            "generation artifact mismatch: " + name,
        )
    if lane == identifier_osa1_absence_suite.TARGET_LANE:
        source_name = "source-no-answer-content-suite.json"
        _require(source_name in paths, "missing generation source suite")
        source_path = manifest_path.parent / source_name
        source_raw = source_path.read_bytes()
        _require(
            evaluator.digest(source_raw)
            == paths[source_name]
            == manifest.get("source_suite_sha256"),
            "generation source suite mismatch",
        )
        source_suite = evaluator.read_json(source_path)
        _require(
            suite["suite_id"]
            == source_suite["suite_id"] + identifier_osa1_absence_suite.SUITE_SUFFIX
            and source_suite["repository_commit"] == suite["repository_commit"]
            and "lexical" in source_suite["routes"]
            and suite["routes"] == ["lexical"]
            and len(source_suite["tasks"]) == len(suite["tasks"])
            and all(
                {**source_task, "source_oracle": target_task["source_oracle"]} == target_task
                and source_task["source_oracle"]
                == {
                    "contract": identifier_osa1_absence_suite.SOURCE_CONTRACT,
                    "unit": "distinct_file",
                }
                for source_task, target_task in zip(
                    source_suite["tasks"], suite["tasks"], strict=True
                )
            ),
            "target suite is not the frozen source population with a new oracle",
        )
        params = manifest.get("parameters")
        source_rows = census["lanes"][identifier_osa1_absence_suite.SOURCE_LANE]["records"]
        _require(
            isinstance(params, dict)
            and params.get("source_lane") == identifier_osa1_absence_suite.SOURCE_LANE
            and params.get("source_admitted") == len(source_rows) == len(source_suite["tasks"])
            and params.get("target_contract")
            == source_oracle.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD,
            "identifier OSA1 generation parameters mismatch",
        )
    if lane in ("no-answer-content", "typo-content-absence"):
        params = manifest.get("parameters")
        _require(isinstance(params, dict), "missing generation parameters")
        if lane == "no-answer-content":
            count = params.get("no_answer")
            _require(
                type(count) is int and count == len(census["lanes"]["no-answer"]["records"]),
                "content no-answer source population mismatch",
            )
        else:
            source = census["lanes"]["typo"]
            _require(
                census["lanes"][lane]["source_admitted"]
                == source["admitted"]
                == sum(row["status"] == "admitted" for row in source["records"]),
                "typo absence source population mismatch",
            )
    return evaluator.digest(raw)


def _policy(capture: dict) -> dict[str, str | None]:
    system = capture["system"]
    profile = capture["execution_profile"]
    if system == "quanta":
        name = profile["policy"]
        _require(name in query_plan.FILE_PROJECTION_ORDERING, "unsupported Quanta file policy")
        return {
            "policy": name,
            "case": (
                "folded"
                if name in query_plan.CODE_SEARCH_FILE_POLICIES
                else "normalizer_defined"
                if name == "literal_file"
                else "sensitive"
            ),
            "scope": (
                "content_and_path"
                if name in ("keyword_file", "code_search_file")
                else "identifier"
                if name == "code_search_typo_file"
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
    route: str | None = None,
) -> dict[str, Any]:
    """Return a source-bound breakdown using evaluator-supplied hit values only."""
    paired = diagnostic.get("report_scope") == "paired_independent_file_judgment_diagnostic_v1"
    _require(
        paired
        or diagnostic.get("report_scope") == "single_route_independent_judgment_diagnostic_v1",
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
    if paired:
        _require(
            route is not None
            and set(suite.get("routes", []))
            == {diagnostic.get("baseline_route"), diagnostic.get("candidate_route")}
            and route in suite["routes"],
            "paired diagnostic requires an explicit declared route",
        )
    else:
        _require(route is None or route == diagnostic["route"], "diagnostic route mismatch")
        route = diagnostic["route"]
        _require(suite.get("routes") == [route], "expected one matching route")
    _require(
        diagnostic.get("route_provenance") == record.get("route_provenance"),
        "route provenance mismatch",
    )
    _require(diagnostic.get("captures") == record.get("captures"), "capture mismatch")
    capture = record["captures"][record["route_provenance"][route]["capture_id"]]
    policy = _policy(capture)

    lane_data = _lane_data(census, lane)
    tasks = _unique([t for t in suite["tasks"] if t["split"] == "eval"], "suite")
    _require(len(tasks) == len(suite["tasks"]), "non-eval task in diagnostic suite")
    declared_contracts = [task.get("evaluation_contract") for task in tasks.values()]
    if any(value is not None for value in declared_contracts):
        _require(
            all(value is not None for value in declared_contracts)
            and all(value == declared_contracts[0] for value in declared_contracts),
            "mixed or missing evaluation contract",
        )
    declared_contract = declared_contracts[0] if declared_contracts else None
    if lane.startswith("default-typo-"):
        _require(
            declared_contract
            == {
                "request_mode": "default_file_search",
                "gold_unit": "distinct_file",
                "result_unit": "distinct_file",
            }
            and paired
            and policy["policy"] in {"code_search_file", "semble:lexical-file"},
            "default robustness projection request or route mismatch",
        )
    if policy["policy"] == "code_search_typo_file" and any(
        not task["answerable"] for task in tasks.values()
    ):
        _require(
            lane_data["contract"] == source_oracle.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD
            and all(
                task["answerable"]
                or task["source_oracle"]["contract"]
                == source_oracle.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD
                for task in tasks.values()
            ),
            "typo no-answer requires independent ascii_identifier_osa1_absent_casefold_v1 oracle",
        )
    if not paired:
        _require(
            diagnostic.get("selected_eval_tasks") == len(tasks), "selected task count mismatch"
        )
    results = _unique([row for row in record["results"] if row["route"] == route], "record")
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
    if lane in NEGATIVE_LANES:
        _require(len(records) == lane_data["admitted"], "content no-answer admission mismatch")
        _require(set(records) == set(tasks), "content no-answer census/suite mismatch")
        source_records = {
            task_id: row
            for task_id, row in _unique(
                census["lanes"][lane_data["derived_from"]]["records"], "source census"
            ).items()
            if row["status"] == "admitted"
        }
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
            if lane_data.get("gold_kind") == "intended_original_name":
                near_names = row["source_partition"]["near_declaration_names"]
                expected_class = "unique" if len(near_names) == 1 else "ambiguous"
                _require(
                    row["intended_name"] == task["intended_name"]
                    and row["source_partition"]["intended_base_name"] == task["intended_name"],
                    "intended name mismatch",
                )
            else:
                expected_class = (
                    "no_answer"
                    if row["matched_names"] == 0
                    else "unique"
                    if row["matched_names"] == 1
                    else "ambiguous"
                )
            _require(
                row.get("answer_class", expected_class) == expected_class,
                "answer class mismatch",
            )
            _require(
                (row.get("answer_class", expected_class) == "no_answer")
                == (not task["answerable"]),
                "answer class mismatch",
            )
            _require(row["gold_files"] == len(task["file_judgments"]), "census gold count mismatch")
        requested = len(records)
        not_admitted = requested - len(admitted)
        strata = {
            task_id: (
                "unique"
                if len(row["source_partition"]["near_declaration_names"]) == 1
                else "ambiguous"
            )
            if lane_data.get("gold_kind") == "intended_original_name"
            else row["answer_class"]
            for task_id, row in admitted.items()
            if task_id in tasks
        }
        contract = lane_data.get("contract", lane_data.get("scoring_contract"))
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
    scored_rows = _unique(
        [row for row in judgments["per_query"] if row["route"] == route],
        "diagnostic per-query",
    )
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
    no_answer_ids = set(tasks) - answerable
    abstained = sum(results[task_id]["status"] == "abstained" for task_id in no_answer_ids)
    no_answer_report = (
        diagnostic["no_answer"]["routes"][route] if paired else diagnostic["no_answer"]
    )
    _require(
        sum(no_answer_report["status_counts"].values()) == len(tasks) - len(answerable),
        "no-answer status count mismatch",
    )
    _require(
        dict(Counter(results[task_id]["status"] for task_id in tasks if task_id not in answerable))
        == no_answer_report["status_counts"],
        "no-answer status mismatch",
    )
    _require(set(no_answer_report["task_ids"]) == no_answer_ids, "no-answer task ID mismatch")
    _require(no_answer_report["sample_count"] == len(no_answer_ids), "no-answer count mismatch")
    _require(no_answer_report["abstained"] == abstained, "no-answer abstained count mismatch")
    _require(
        no_answer_report["abstention_rate"]
        == (abstained / len(no_answer_ids) if no_answer_ids else evaluator.NOT_APPLICABLE),
        "no-answer abstention rate mismatch",
    )
    nonempty_no_answer = sum(bool(results[task_id]["candidates"]) for task_id in no_answer_ids)
    _require(
        no_answer_report["nonempty_results"] == nonempty_no_answer,
        "no-answer nonempty result count mismatch",
    )
    _require(
        no_answer_report["nonempty_result_rate"]
        == (nonempty_no_answer / len(no_answer_ids) if no_answer_ids else evaluator.NOT_APPLICABLE),
        "no-answer nonempty result rate mismatch",
    )

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
    admitted_records = {
        task_id: row
        for task_id, row in records.items()
        if row.get("status") == "admitted" and task_id in tasks
    }
    operation_records = {
        task_id: {"operation": row["generation"]["operation"]}
        for task_id, row in admitted_records.items()
        if isinstance(row.get("generation"), dict) and "operation" in row["generation"]
    }
    detailed_strata = {
        "operation": _category_breakdown(operation_records, scored_rows, results, "operation")
    }
    for field in ("length", "short_common", "overcorrection_candidate", "near_name_collision"):
        subset = {
            task_id: {field: row["strata"][field]}
            for task_id, row in admitted_records.items()
            if isinstance(row.get("strata"), dict) and field in row["strata"]
        }
        detailed_strata[field] = _category_breakdown(subset, scored_rows, results, field)
        if subset:
            _require(set(subset) == set(admitted_records), f"incomplete {field} census metadata")
    sampling = (
        _sampling_breakdown(census, records, admitted_records, scored_rows, results)
        if lane != "no-answer" and lane not in NEGATIVE_LANES
        else {}
    )
    if operation_records:
        _require(
            set(operation_records) == set(admitted_records), "incomplete operation census metadata"
        )
    if contract == source_oracle.ASCII_CONTENT_ABSENT_CASEFOLD:
        evaluation_intent = "content_absence_negative_control"
        negative_reference_scope = "folded_content_absent"
    elif contract == source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD:
        evaluation_intent = "code_search_absence_negative_control"
        negative_reference_scope = "folded_content_and_path_absent"
    elif contract == source_oracle.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD:
        evaluation_intent = "identifier_osa1_absence_negative_control"
        negative_reference_scope = "folded_ascii_identifier_osa1_absent"
    elif lane_data.get("gold_kind") == "intended_original_name":
        evaluation_intent = "intended_name_file_retrieval_diagnostic"
        negative_reference_scope = "not_applicable"
    else:
        evaluation_intent = "declaration_name_file_retrieval_diagnostic"
        negative_reference_scope = "declaration_local_name_absent"
    return {
        "status": "diagnostic_unqualified",
        "evaluation_intent": evaluation_intent,
        "evaluation_contract": declared_contract,
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
        "identifier_osa1_absence_replay_verified": False,
        "rank_unit": "distinct_file",
        **policy,
        "requested": requested,
        "not_admitted": not_admitted,
        "unsupported_query_form": len(unsupported) if lane not in NEGATIVE_LANES else 0,
        "submitted": len(tasks),
        "eligible": len(eligible),
        "status_counts": dict(sorted(statuses.items())),
        "no_answer": {
            "negative_reference_scope": negative_reference_scope,
            "sample_count": no_answer_report["sample_count"],
            "abstained": no_answer_report["abstained"],
            "abstention_rate": no_answer_report["abstention_rate"],
            "nonempty_results": no_answer_report["nonempty_results"],
            "nonempty_result_rate": no_answer_report["nonempty_result_rate"],
            "status_counts": no_answer_report["status_counts"],
        },
        "strata": breakdown,
        "detailed_strata": detailed_strata,
        "sampling": sampling,
    }


def compose_validated(
    suite: dict[str, Any],
    pack: dict[str, Any],
    record: dict[str, Any],
    diagnostic: dict[str, Any],
    census: dict[str, Any],
    lane: str,
    route: str | None = None,
) -> dict[str, Any]:
    """Verify the hit/status subset against evaluator replay before joining.

    Historical diagnostics can differ in optional fields and insignificant
    floating-point NDCG digits. This report consumes only hit, eligibility,
    exclusion, and status, so require exact equality for those fields.
    """
    if diagnostic.get("report_scope") == "paired_independent_file_judgment_diagnostic_v1":
        expected = evaluator.evaluate_paired_file_diagnostic(
            suite,
            pack,
            record,
            diagnostic["baseline_route"],
            diagnostic["candidate_route"],
        )
        _require(
            evaluator.canonical(expected) == evaluator.canonical(diagnostic),
            "paired diagnostic differs from evaluator replay",
        )
        return compose(suite, census, record, diagnostic, lane, route)
    expected = evaluator.evaluate_diagnostic(suite, pack, record)
    if any("evaluation_contract" in task for task in suite["tasks"]):
        _require(
            evaluator.canonical(expected) == evaluator.canonical(diagnostic),
            "diagnostic differs from evaluator replay",
        )
        return compose(suite, census, record, diagnostic, lane, route)
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
    return compose(suite, census, record, diagnostic, lane, route)


def compare_paired_clean_typo(
    clean_suite: dict[str, Any],
    clean_diagnostic: dict[str, Any],
    typo_suite: dict[str, Any],
    typo_diagnostic: dict[str, Any],
    census: dict[str, Any],
    lane: str,
    *,
    clean_route: str,
    typo_route: str,
) -> dict[str, Any]:
    """Compare source-paired intended-file judgments; callers replay both diagnostics first."""
    lane_data = _lane_data(census, lane)
    _require(
        lane_data.get("gold_kind") == "intended_original_name",
        "paired robustness requires intended original-name gold",
    )
    _require(
        clean_suite["repository_commit"] == typo_suite["repository_commit"]
        and clean_suite["file_universe"] == typo_suite["file_universe"]
        and clean_suite["comparison_contract"] == typo_suite["comparison_contract"],
        "paired robustness source or comparison contract mismatch",
    )
    clean_tasks = _unique(clean_suite["tasks"], "clean suite")
    typo_tasks = _unique(typo_suite["tasks"], "typo suite")
    census_rows = _unique(lane_data["records"], "paired census")
    admitted = {key: row for key, row in census_rows.items() if row["status"] == "admitted"}
    _require(set(admitted) == set(typo_tasks), "paired census/typo suite mismatch")

    def diagnostic_rows(diagnostic: dict, route: str, tasks: dict[str, dict]) -> dict[str, dict]:
        _require(
            diagnostic.get("status") == "diagnostic_unqualified"
            and diagnostic.get("repository_commit") == clean_suite["repository_commit"]
            and diagnostic.get("comparison_contract") == clean_suite["comparison_contract"],
            "paired diagnostic source or status mismatch",
        )
        judgments = diagnostic["judgment_metrics"]["file_judgments"]
        _require(route in judgments["routes"], "paired diagnostic route missing")
        rows = _unique(
            [row for row in judgments["per_query"] if row["route"] == route],
            "paired diagnostic rows",
        )
        _require(set(rows) == set(tasks), "paired diagnostic task coverage mismatch")
        return rows

    clean_rows = diagnostic_rows(clean_diagnostic, clean_route, clean_tasks)
    typo_rows = diagnostic_rows(typo_diagnostic, typo_route, typo_tasks)
    metrics = ("hit_at_10", "mrr_at_10", "ndcg_at_10")
    sums = {metric: {"clean": 0.0, "typo": 0.0} for metric in metrics}
    excluded: list[dict[str, str]] = []
    paired_families: set[str] = set()
    for task_id, row in admitted.items():
        clean_id = "CLN-" + row["base_task_id"]
        _require(clean_id in clean_tasks, "paired base task missing")
        clean_task, typo_task = clean_tasks[clean_id], typo_tasks[task_id]
        family = typo_task["query_family_id"]
        _require(
            family == clean_task["query_family_id"] == row["query_family_id"]
            and family not in paired_families
            and clean_task["query"] == row["base_query"] == typo_task["intended_name"]
            and typo_task["query"] == row["query"]
            and clean_task["file_judgments"] == typo_task["file_judgments"],
            "paired family or intended gold mismatch",
        )
        paired_families.add(family)
        clean_score, typo_score = clean_rows[clean_id], typo_rows[task_id]
        if not clean_score["eligible"] or not typo_score["eligible"]:
            excluded.append(
                {
                    "query_family_id": family,
                    "reason": "clean_" + clean_score.get("reason", "eligible")
                    if not clean_score["eligible"]
                    else "typo_" + typo_score.get("reason", "eligible"),
                }
            )
            continue
        for metric in metrics:
            clean_value = clean_score["scores"].get(metric)
            typo_value = typo_score["scores"].get(metric)
            _require(
                type(clean_value) in (int, float)
                and type(typo_value) in (int, float)
                and 0 <= clean_value <= 1
                and 0 <= typo_value <= 1,
                "paired metric missing or invalid: " + metric,
            )
            sums[metric]["clean"] += clean_value
            sums[metric]["typo"] += typo_value
    count = len(admitted) - len(excluded)
    return {
        "status": "diagnostic_unqualified",
        "lane": lane,
        "gold_kind": "intended_original_name",
        "admitted_families": len(admitted),
        "paired_eligible_families": count,
        "excluded": sorted(excluded, key=lambda row: row["query_family_id"]),
        "metrics": {
            metric: {
                "clean_mean": values["clean"] / count if count else evaluator.NOT_APPLICABLE,
                "typo_mean": values["typo"] / count if count else evaluator.NOT_APPLICABLE,
                "delta_typo_minus_clean": (
                    (values["typo"] - values["clean"]) / count
                    if count
                    else evaluator.NOT_APPLICABLE
                ),
            }
            for metric, values in sums.items()
        },
    }


def _verify_paired_capture_identity(
    clean_record: dict[str, Any],
    typo_record: dict[str, Any],
    clean_route: str,
    typo_route: str,
) -> None:
    def capture(record: dict, route: str) -> dict:
        return record["captures"][record["route_provenance"][route]["capture_id"]]

    clean, typo = capture(clean_record, clean_route), capture(typo_record, typo_route)
    for field in (
        "system",
        "runner_binary",
        "searchd_binary",
        "model",
        "model_revision",
        "chunk_strategy",
        "chunk_config",
        "generation",
    ):
        _require(clean.get(field) == typo.get(field), "paired capture differs in " + field)
    # Activation receipts name separate state roots. Their digests must be
    # present and well formed, but equality would reject independent rebuilds
    # of the same source universe. Source bytes and file universe are checked
    # by both evaluator replays and compare_paired_clean_typo.
    for value in (clean.get("activation_digest"), typo.get("activation_digest")):
        _require(
            isinstance(value, str)
            and len(value) == 64
            and all(char in "0123456789abcdef" for char in value),
            "paired capture has invalid activation digest",
        )


def verify_census_against_source(
    repo: Path, suite: dict[str, Any], census: dict[str, Any], lane: str
) -> None:
    """Independently rederive the ambiguity strata from the frozen Go files."""
    if lane in ("no-answer-content", identifier_osa1_absence_suite.TARGET_LANE):
        # NOC task labels are replayed by validate_suite. Archived NOC used a
        # declaration-only oracle and remains explicitly unverified.
        return
    source = evaluator.SourceSnapshot(repo, suite["repository_commit"])
    files = {
        item["path"]: (source.file(item["path"])[0], item["file_sha256"])
        for item in suite["file_universe"]
    }
    if lane == "typo-content-absence":
        lane_data = census["lanes"][lane]
        source_rows = {
            row["task_id"]: row
            for row in census["lanes"]["typo"]["records"]
            if row["status"] == "admitted"
        }
        oracle = source_oracle.SourceOracleIndex(
            files, {row["query"] for row in source_rows.values()}
        )
        expected_admitted: set[str] = set()
        expected_excluded: dict[str, str] = {}
        for task_id, row in source_rows.items():
            try:
                oracle.expected_rows(
                    source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD,
                    row["query"],
                    "distinct_file",
                )
            except source_oracle.SourceOracleError as exc:
                expected_excluded[task_id] = str(exc)
            else:
                expected_admitted.add(task_id)
        mapped = [row["source_task_id"] for row in lane_data["records"]]
        excluded = lane_data["excluded_probes"]
        _require(
            len(mapped) == len(set(mapped)) and set(mapped) == expected_admitted,
            "typo absence admitted source mismatch",
        )
        _require(
            len(excluded) == len(expected_excluded)
            and {row["source_task_id"]: (row["query"], row["reason"]) for row in excluded}
            == {
                task_id: (source_rows[task_id]["query"], reason)
                for task_id, reason in expected_excluded.items()
            },
            "typo absence excluded source mismatch",
        )
        return
    lane_data = _lane_data(census, lane)
    contract = lane_data.get("contract", lane_data.get("scoring_contract"))
    _require(
        contract in source_oracle.DECLARATION_NAME_CONTRACTS,
        "census contract is not a declaration name contract",
    )
    if lane_data.get("gold_kind") == "intended_original_name":
        language, variant = source_oracle.NAME_CONTRACTS[contract]
        _require(variant == "exact", "intended-name scoring contract must be exact")
        near = lane_data.get("near_declaration_metadata_contract")
        _require(
            source_oracle.NAME_CONTRACTS.get(near) == (language, "osa1_casefold"),
            "intended-name near contract mismatch",
        )
        admitted_rows = [row for row in lane_data["records"] if row["status"] == "admitted"]
        # The oracle bounds query names per instance. Replay the full census in
        # bounded batches without dropping any admitted task from verification.
        for start in range(0, len(admitted_rows), 500):
            batch = admitted_rows[start : start + 500]
            names = {name for row in batch for name in (row["query"], row["intended_name"])}
            oracle = source_oracle.SourceOracleIndex(files, names)
            for row in batch:
                partition = oracle.typo_gold_partition(language, row["query"], row["intended_name"])
                _require(
                    row["source_partition"] == partition
                    and row["gold_files"] == len(partition["intended_base_files"])
                    and row["matched_names"] == 1,
                    "census/source intended-name partition mismatch",
                )
        return
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
    for name in ("repo", "suite", "record", "diagnostic", "census", "lane", "generation-manifest"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--route")
    parser.add_argument("--clean-suite", type=Path)
    parser.add_argument("--clean-record", type=Path)
    parser.add_argument("--clean-diagnostic", type=Path)
    parser.add_argument("--clean-route")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        suite, pack, record = evaluator.load_evidence(
            Path(args.repo).resolve(), Path(args.suite), Path(args.record)
        )
        census = evaluator.read_json(Path(args.census))
        diagnostic = evaluator.read_json(Path(args.diagnostic))
        manifest_sha256 = verify_generation_manifest(
            Path(args.generation_manifest),
            Path(args.census),
            Path(args.suite),
            suite,
            census,
            args.lane,
        )
        verify_census_against_source(Path(args.repo).resolve(), suite, census, args.lane)
        output = compose_validated(suite, pack, record, diagnostic, census, args.lane, args.route)
        clean_inputs = (args.clean_suite, args.clean_record, args.clean_diagnostic)
        if any(value is not None for value in clean_inputs):
            _require(
                all(value is not None for value in clean_inputs), "incomplete clean pair inputs"
            )
            clean_suite, clean_pack, clean_record = evaluator.load_evidence(
                Path(args.repo).resolve(), args.clean_suite, args.clean_record
            )
            clean_diagnostic = evaluator.read_json(args.clean_diagnostic)
            clean_route = args.clean_route or clean_diagnostic.get("route")
            typo_route = args.route or diagnostic.get("route")
            _require(
                isinstance(clean_route, str) and isinstance(typo_route, str),
                "paired routes must be explicit",
            )
            expected_clean = evaluator.evaluate_diagnostic(clean_suite, clean_pack, clean_record)
            _require(
                evaluator.canonical(expected_clean) == evaluator.canonical(clean_diagnostic),
                "clean diagnostic differs from evaluator replay",
            )
            _verify_paired_capture_identity(clean_record, record, clean_route, typo_route)
            output["paired_clean_typo"] = compare_paired_clean_typo(
                clean_suite,
                clean_diagnostic,
                suite,
                diagnostic,
                census,
                args.lane,
                clean_route=clean_route,
                typo_route=typo_route,
            )
        output["generation_manifest_sha256"] = manifest_sha256
        output["content_absence_replay_verified"] = (
            args.lane == "no-answer-content"
            and output["source_oracle_contract"] == source_oracle.ASCII_CONTENT_ABSENT_CASEFOLD
        ) or (
            args.lane == "typo-content-absence"
            and output["source_oracle_contract"] == source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD
        )
        output["identifier_osa1_absence_replay_verified"] = (
            args.lane == identifier_osa1_absence_suite.TARGET_LANE
            and output["source_oracle_contract"]
            == source_oracle.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD
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
