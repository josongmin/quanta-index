"""Validate native, record-bound complete-pool ablations; never select defaults.

The capture contains no gold. This reader joins a separately validated suite and
uses the existing file evaluator. A stopped walk, refused explain, unknown
declaration census, or unsupported engine cannot become a zero or a complete
ranking. Diagnostic measurements are separate from original query latency.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
from pathlib import Path

try:
    from tools.benchmark.retrieval import evaluator as ev
    from tools.benchmark.retrieval import query_plan as qp
except ImportError:  # Sibling import from the existing direct-script driver.
    import evaluator as ev
    import query_plan as qp

POLICIES = (
    "baseline",
    "declaration_only",
    "boundary_only",
    "occurrence_half",
    "occurrence_none",
    "combined",
)
ORDINARY_SCOPE = "code_search.execution.scope=ordinary_exhaustive_page_v1;exploration_complete=true"


def require(ok: bool, message: str) -> None:
    if not ok:
        raise ValueError(message)


def _uint(value: object, where: str) -> int:
    require(type(value) is int and 0 <= value <= 2**64 - 1, f"{where}: invalid unsigned integer")
    return value


def _number(value: object, where: str) -> float:
    require(
        type(value) in (int, float) and math.isfinite(value) and value >= 0,
        f"{where}: invalid finite number",
    )
    return float(value)


def _details(explanation: dict) -> list[str]:
    rows = explanation["planner_trace"]
    require(isinstance(rows, list), "native planner trace must be a list")
    return [row["detail"] for row in rows if row["stage"] == "merge"]


def _count(details: list[str], name: str) -> int:
    values = [
        text.removeprefix(f"code_search.execution.{name}=")
        for text in details
        if text.startswith(f"code_search.execution.{name}=")
    ]
    require(
        len(values) == 1 and re.fullmatch(r"[0-9]+", values[0]) is not None,
        f"missing/duplicate/invalid count {name}",
    )
    return _uint(int(values[0]), name)


def _order(candidate: dict, score: float | None = None) -> tuple:
    return (
        -_number(candidate["score"] if score is None else score, "candidate score"),
        candidate["source_repo_id"],
        candidate["repo_relative_path"],
        candidate["start_line"],
        candidate["end_line"],
        candidate["candidate_id"],
    )


def _authority(candidate: dict, pin: dict) -> None:
    require(
        candidate["source_repo_id"] == candidate["repo_id"] == pin["repo_id"]
        and candidate["revision_id"] == pin["revision_id"]
        and candidate["manifest_generation"] == pin["manifest_generation"],
        "candidate generation/source mismatch",
    )
    source = candidate["source"]
    require(
        isinstance(source, dict)
        and source["revision_id"] == pin["revision_id"]
        and source["file"]
        == {
            "source_repo_id": pin["repo_id"],
            "repo_relative_path": candidate["repo_relative_path"],
        },
        "candidate source-file identity mismatch",
    )
    digest = source["source_sha256"]
    require(
        isinstance(digest, list)
        and len(digest) == 32
        and all(type(byte) is int and 0 <= byte <= 255 for byte in digest),
        "invalid source digest",
    )
    path = candidate["repo_relative_path"]
    require(
        isinstance(path, str)
        and bool(path)
        and not Path(path).is_absolute()
        and not set(path.split("/")) & {".", "..", ""},
        "invalid file path",
    )
    require(
        candidate["candidate_id"].startswith("file:") and candidate["preview"] is not None,
        "candidate lacks file authority",
    )
    _order(candidate)


def _pool(row: dict, original: dict) -> list[dict]:
    collection = row["collection"]
    pages = collection["pages"]
    require(isinstance(pages, list), "pages must be a list")
    require(type(collection["pool_complete"]) is bool, "pool completeness must be a boolean")
    _number(collection["diagnostic_ms"], "diagnostic_ms")
    if not pages:
        require(not collection["pool_complete"], "empty evidence cannot prove a complete pool")
        return []
    first = pages[0]
    require(
        original["status"] in ("success", "abstained", "capped")
        and (first["window"]["outcome"]["kind"] == "exact_exhausted")
        == (original["status"] != "capped"),
        "first page exhaustion differs from original result status",
    )
    original_candidates = original["candidates"]
    require(
        len(first["results"]) == len(original_candidates), "first page differs from original window"
    )
    for candidate, captured in zip(first["results"], original_candidates, strict=True):
        require(
            candidate["candidate_id"] == captured["span_accounting"]["unit_id"]
            and candidate["repo_relative_path"] == captured["path"]
            and candidate["score"] == captured["score"]
            and candidate["source_repo_id"] == captured["span_accounting"]["source_repo_id"]
            and candidate["revision_id"] == captured["span_accounting"]["source_revision_id"]
            and bytes(candidate["source"]["source_sha256"]).hex() == captured["file_sha256"],
            "first page differs from record file authority",
        )
    candidates: list[dict] = []
    ids: set[str] = set()
    paths: set[str] = set()
    total: int | None = None
    for index, page in enumerate(pages):
        require(
            page["generation"] == row["generation"] and page["rank_unit"] == "file",
            "page generation/unit mismatch",
        )
        details = _details(page["explanation"])
        require(
            details.count(ORDINARY_SCOPE) == 1, "ordinary exhaustive exploration not established"
        )
        modes = [
            detail.removeprefix("code_search.execution.mode=")
            for detail in details
            if detail.startswith("code_search.execution.mode=")
        ]
        # Older bound captures predate the mode trace. If a producer emits it,
        # the mode must agree with the ordinary complete-pool contract.
        require(
            not modes or modes == ["ordinary"],
            "ordinary execution mode is malformed or contradictory",
        )
        verified = _count(details, "verified_matching_files")
        eligible = _count(details, "cursor_eligible_files")
        returned = len(page["results"])
        require(total is None or total == verified, "file universe changed between pages")
        total = verified
        require(
            eligible == total - len(candidates)
            and returned <= eligible
            and _count(details, "returned_files") == page["window"]["returned"] == returned,
            "page counts disagree with full pool",
        )
        if index:
            require(
                pages[index - 1].get("next_cursor") is not None,
                "paging continued without native cursor",
            )
        for candidate in page["results"]:
            _authority(candidate, row["generation"])
            require(
                candidate["candidate_id"] not in ids
                and candidate["repo_relative_path"] not in paths,
                "duplicate file in complete pool",
            )
            require(
                not candidates or _order(candidates[-1]) < _order(candidate),
                "native paging order contradiction",
            )
            ids.add(candidate["candidate_id"])
            paths.add(candidate["repo_relative_path"])
            candidates.append(candidate)
    if collection["pool_complete"]:
        last = pages[-1]
        require(
            last["window"]["outcome"]["kind"] == "exact_exhausted"
            and last.get("next_cursor") is None
            and total == len(candidates),
            "false complete-pool exhaustion",
        )
    return candidates


def _scores(candidate: dict, response: dict, pin: dict) -> tuple[dict[str, int], bool]:
    require(
        response["generation"] == pin and response["presence"] == "indexed",
        "explain generation/presence mismatch",
    )
    explanation = response["explanation"]
    details = _details(explanation)
    require(details.count("explain.score_reconciled=true") == 1, "native score did not reconcile")
    components: dict[str, int] = {}
    study: dict[str, int] = {}
    declaration: int | None = None
    coverage: bool | None = None
    boundary: int | None = None
    for detail in details:
        match = re.fullmatch(
            r"explain\.code_search_score\.(boundary_and_path|occurrence|exact_case|proximity)=([0-9]+)",
            detail,
        )
        if match:
            require(match[1] not in components, "duplicate score component")
            components[match[1]] = _uint(int(match[2]), "score component")
        match = re.fullmatch(
            r"explain\.code_search_rank_study_v1\.(\w+)=([0-9]+);selected=(true|false)", detail
        )
        if match:
            require(
                match[1] in POLICIES
                and match[1] not in study
                and (match[3] == "true") == (match[1] == "baseline"),
                "unknown, duplicate or selected experimental policy",
            )
            study[match[1]] = _uint(int(match[2]), "study score")
        match = re.fullmatch(
            r"explain\.code_search_rank_study_v1\.declaration_bonus=(unknown|[0-9]+);coverage_complete=(true|false);original_boundary_bonus=([0-9]+)",
            detail,
        )
        if match:
            require(coverage is None, "duplicate declaration authority")
            declaration = (
                None if match[1] == "unknown" else _uint(int(match[1]), "declaration bonus")
            )
            coverage = match[2] == "true"
            boundary = _uint(int(match[3]), "boundary bonus")
    require(
        set(components) == {"boundary_and_path", "occurrence", "exact_case", "proximity"},
        "incomplete selected score decomposition",
    )
    baseline = sum(components.values())
    require(
        baseline == candidate["score"],
        "native baseline contradicts selected score",
    )
    contributions = explanation["contributions"]
    require(
        len(contributions) == 1
        and contributions[0]["signal_name"] == "lexical.code_search_file"
        and set(contributions[0]) == {"signal_name", "signal_value", "weight", "contribution"}
        and _number(contributions[0]["signal_value"], "native signal") == baseline
        and _number(contributions[0]["weight"], "native weight") == 1.0
        and _number(contributions[0]["contribution"], "native contribution") == baseline,
        "native total contribution mismatch",
    )
    refusals = [
        detail
        for detail in details
        if detail.startswith("explain.code_search_rank_study_v1.refused=")
    ]
    if refusals:
        require(
            refusals
            == ["explain.code_search_rank_study_v1.refused=LEXICAL_COLLECTION_BUDGET_EXCEEDED"]
            and not study
            and boundary is None
            and coverage is None
            and sum("code_search_rank_study_v1." in detail for detail in details) == 1,
            "invalid or contradictory native diagnostic refusal",
        )
        # The native selected score is proven, but no experimental score exists.
        # Keep the task excluded rather than substituting neutral/zero features.
        return {}, False
    require(
        set(study) == set(POLICIES) and boundary is not None and coverage is not None,
        "incomplete native ordinary score study",
    )
    require(study["baseline"] == baseline, "native baseline contradicts selected score")
    require(
        not coverage or declaration is not None, "complete declaration census cannot be unknown"
    )
    without = baseline - components["occurrence"]

    def sat(value):
        return min(value, 2**32 - 1)

    require(
        study
        == {
            "baseline": baseline,
            "declaration_only": sat(baseline + (declaration or 0)),
            "boundary_only": sat(baseline + boundary),
            "occurrence_half": sat(without + components["occurrence"] // 2),
            "occurrence_none": without,
            "combined": sat(without + (declaration or 0) + boundary),
        },
        "native ablation algebra contradiction",
    )
    return study, coverage


def validate_artifact(
    artifact: dict, record: dict, record_sha256: str, pack: dict
) -> dict[str, dict]:
    """Return validated rows. The caller must validate the original record first."""
    require(
        artifact["schema_version"] == 1
        and artifact["kind"] == "quanta_code_search_rank_study"
        and artifact["qualification"] == "diagnostic_unqualified",
        "invalid rank-study contract",
    )
    require(
        re.fullmatch(r"[0-9a-f]{64}", record_sha256) is not None
        and artifact["record_sha256"] == record_sha256,
        "study is not bound to original record bytes",
    )
    policy = artifact["policy"]
    require(
        policy in ("code_search_file", "code_search_exact_content_file"),
        "unsupported rank-study policy",
    )
    capture = record["captures"][record["route_provenance"]["lexical"]["capture_id"]]
    require(
        "source_repo_id" in capture and "source_revision_id" in capture,
        "rank study requires original capture source identity, including zero-hit tasks",
    )
    require(
        capture["execution_profile"] == qp.execution_profile(policy)
        and artifact["execution_profile_sha256"]
        == capture["execution_profile_sha256"]
        == qp.execution_profile_sha256(policy),
        "rank-study execution profile mismatch",
    )
    require(
        artifact["timing_boundary"] == "post_measurement_sdk_paging_and_explanations",
        "study cost must be outside query measurements",
    )
    _number(artifact["diagnostic_ms"], "diagnostic_ms")
    for name, maximum in (("max_files", 100_000), ("max_pages", 10_000), ("timeout_ms", 300_000)):
        require(
            0 < _uint(artifact["limits"][name], name) <= maximum,
            "study limit is outside producer bounds",
        )
    originals = {row["task_id"]: row for row in record["results"] if row["route"] == "lexical"}
    tasks = {task["task_id"]: task for task in pack["tasks"]}
    universe = {item["path"]: item["file_sha256"] for item in pack["file_universe"]}
    validated: dict[str, dict] = {}
    for row in artifact["results"]:
        task_id = row["task_id"]
        require(
            task_id in originals
            and task_id in tasks
            and task_id not in validated
            and row["route"] == "lexical",
            "study task inventory mismatch",
        )
        original = originals[task_id]
        derived = qp.derive_query_identity(policy, tasks[task_id]["query"])
        require(
            all(
                row["query_identity"][name] == value == original["query_identity"][name]
                for name, value in derived.items()
            ),
            "query identity mismatch",
        )
        require(
            row["query_identity"]["policy_config_sha256"]
            == hashlib.sha256(qp.policy_config_canonical(policy).encode()).hexdigest(),
            "policy config digest mismatch",
        )
        pin = row["generation"]
        require(
            pin["manifest_generation"] == capture["generation"]
            and pin["repo_id"] == capture["source_repo_id"]
            and pin["revision_id"] == capture["source_revision_id"],
            "study generation/source differs from original capture",
        )
        request = row["effective_request"]
        require(
            request["syntax"] == "code_search"
            and request["generation"] == pin
            and request["query_text"] == qp.plan_lexical_request(policy, tasks[task_id]["query"])
            and request["top_k"] == record["comparison_contract"]["top_k"]
            and request["constraints"] == {"language_any_of": []}
            and request.get("cursor") is None
            and request.get("generation_selector") is None,
            "effective request mismatch",
        )
        collection = row["collection"]
        require(
            collection["status"] in ("returned", "partial", "not_run"), "invalid collection status"
        )
        if collection["status"] != "returned":
            require(
                isinstance(collection["reason"], str) and bool(collection["reason"]),
                "incomplete study needs a reason",
            )
            require(
                type(collection["pool_complete"]) is bool, "pool completeness must be a boolean"
            )
            require(
                len(collection["pages"]) <= artifact["limits"]["max_pages"], "page limit exceeded"
            )
            _number(collection["diagnostic_ms"], "diagnostic_ms")
            # Incomplete evidence is retained for inspection, never normalized
            # into a scoreable pool. Unsupported recovery can lack ordinary
            # count/feature traces even when its original quality row is valid.
            validated[task_id] = {"collection": collection, "candidates": [], "explained": {}}
            continue
        candidates = _pool(row, original)
        for candidate in candidates:
            require(
                universe.get(candidate["repo_relative_path"])
                == bytes(candidate["source"]["source_sha256"]).hex(),
                "complete pool contains a file outside the source-bound universe",
            )
        require(
            collection["reason"] is None and collection["pool_complete"],
            "returned study lacks complete pool",
        )
        require(len(collection["pages"]) <= artifact["limits"]["max_pages"], "page limit exceeded")
        require(len(candidates) <= artifact["limits"]["max_files"], "file limit exceeded")
        explained: dict[str, tuple[dict, bool]] = {}
        by_id = {candidate["candidate_id"]: candidate for candidate in candidates}
        for item in collection["explanations"]:
            candidate = item["candidate"]
            candidate_id = candidate["candidate_id"]
            require(
                candidate_id in by_id
                and candidate == by_id[candidate_id]
                and candidate_id not in explained,
                "explanation candidate differs from native pool",
            )
            _number(item["explanation_ms"], "explanation_ms")
            if item["status"] == "refused":
                require(
                    isinstance(item["error"], str) and bool(item["error"]),
                    "refusal needs original error",
                )
                explained[candidate_id] = ({}, False)
            else:
                require(item["status"] == "returned", "invalid explanation status")
                explained[candidate_id] = _scores(candidate, item["response"], pin)
        validated[task_id] = {
            "collection": collection,
            "candidates": candidates,
            "explained": explained,
        }
    require(set(validated) == set(originals) == set(tasks), "study omitted original tasks")
    return validated


def _paired_means(samples: list[dict]) -> dict:
    return {
        side: {
            name: sum(sample[side][name] for sample in samples) / len(samples)
            if samples
            else None
            for name in ("file_ndcg", "file_hit", "file_mrr")
        }
        for side in ("baseline", "candidate")
    }


def _regressions(samples: list[dict]) -> dict:
    return {
        metric: [
            sample["task_id"]
            for sample in samples
            if sample["candidate"][metric] < sample["baseline"][metric]
        ]
        for metric in ("file_ndcg", "file_hit", "file_mrr")
    }


def _family_summary(tasks: dict, samples: list[dict]) -> dict:
    if any(not task.get("query_family_id") for task in tasks.values()):
        return {"status": "not_available", "reason": "query_family_identity_absent"}
    groups: dict[str, list[dict]] = {}
    for sample in samples:
        family = tasks[sample["task_id"]]["query_family_id"]
        groups.setdefault(family, []).append(sample)
    means = [_paired_means(group) for group in groups.values()]
    return {
        "status": "admitted_families_only",
        "family_count": len(groups),
        "paired_means": {
            side: {
                metric: sum(mean[side][metric] for mean in means) / len(means) if means else None
                for metric in ("file_ndcg", "file_hit", "file_mrr")
            }
            for side in ("baseline", "candidate")
        },
    }


def _intent_comparisons(tasks: dict, samples: list[dict], excluded: list[dict]) -> list[dict]:
    # A literal-content oracle and a declaration oracle may use the same file
    # metric but have different relevance contracts. Never hide them in one mean.
    groups: dict[tuple[str, str], set[str]] = {}
    for task_id, task in tasks.items():
        key = (
            task.get("query_intent", "unspecified"),
            task.get("source_oracle", {}).get("contract", "unspecified"),
        )
        groups.setdefault(key, set()).add(task_id)
    result = []
    for (intent, contract), ids in sorted(groups.items()):
        admitted = [sample for sample in samples if sample["task_id"] in ids]
        exclusions = [row for row in excluded if row["task_id"] in ids]
        require(
            len(admitted) + len(exclusions) == len(ids),
            "intent comparison omitted a task",
        )
        result.append(
            {
                "query_intent": intent,
                "source_oracle_contract": contract,
                "task_count": len(ids),
                "eligible_task_ids": [sample["task_id"] for sample in admitted],
                "excluded": exclusions,
                "coverage": len(admitted) / len(ids),
                "paired_means": _paired_means(admitted),
                "regressions": _regressions(admitted),
                "family_macro": _family_summary(
                    {task_id: tasks[task_id] for task_id in ids}, admitted
                ),
            }
        )
    return result


def compose(suite: dict, rows: dict[str, dict]) -> dict:
    """Use common eligible tasks for each baseline/candidate comparison."""
    tasks = {task["task_id"]: task for task in suite["tasks"]}
    require(set(tasks) == set(rows), "rank study task roster differs from suite")
    comparisons = {}
    k = suite["comparison_contract"]["top_k"]
    for policy in POLICIES[1:]:
        admitted = []
        excluded = []
        samples = []
        for task_id, row in rows.items():
            collection = row["collection"]
            explained = row["explained"]
            reason = None
            if collection["status"] != "returned" or not collection["pool_complete"]:
                reason = collection["reason"] or "pool_incomplete"
            elif set(explained) != {
                candidate["candidate_id"] for candidate in row["candidates"]
            } or any(not value[0] for value in explained.values()):
                reason = "explanation_incomplete_or_refused"
            elif policy in ("declaration_only", "combined") and any(
                not value[1] for value in explained.values()
            ):
                reason = "declaration_census_unknown_or_incomplete"
            elif "file_judgments" not in tasks[task_id]:
                reason = "independent_file_judgments_absent"
            elif not any(judgment["grade"] > 0 for judgment in tasks[task_id]["file_judgments"]):
                reason = "no_answer_uses_separate_abstention_metrics"
            if reason:
                excluded.append({"task_id": task_id, "reason": reason})
                continue
            admitted.append(task_id)
            baseline = row["candidates"]
            candidate = sorted(
                baseline, key=lambda item: _order(item, explained[item["candidate_id"]][0][policy])
            )
            judgments = tasks[task_id]["file_judgments"]

            def metrics(items, judgments=judgments):
                files = [{"path": item["repo_relative_path"]} for item in items]
                return {
                    "file_ndcg": ev.file_ndcg_at_k(files, judgments, k),
                    "file_hit": ev.file_hit_at_k_judged(files, judgments, k),
                    "file_mrr": ev.file_mrr_at_k_judged(files, judgments, k),
                }

            samples.append(
                {
                    "task_id": task_id,
                    "baseline": metrics(baseline),
                    "candidate": metrics(candidate),
                    "baseline_top_k": [item["repo_relative_path"] for item in baseline[:k]],
                    "candidate_top_k": [item["repo_relative_path"] for item in candidate[:k]],
                }
            )
        comparisons[policy] = {
            "eligible_task_ids": admitted,
            "excluded": excluded,
            "paired_means": _paired_means(samples),
            "coverage": len(admitted) / len(rows) if rows else 0.0,
            "samples": samples,
            "regressions": _regressions(samples),
            "intent_comparisons": _intent_comparisons(tasks, samples, excluded),
            "family_macro": _family_summary(tasks, samples),
        }
    return {
        "kind": "code_search_complete_pool_ablation_report",
        "qualification": "diagnostic_unqualified",
        "selected_policy": "baseline",
        "rank_unit": "distinct_file",
        "top_k": k,
        "declaration_span_metrics": "not_applicable_file_level_features",
        "comparisons": comparisons,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("repo", "suite", "record", "study", "out"):
        parser.add_argument(f"--{flag}", type=Path, required=True)
    args = parser.parse_args()
    suite, pack, record = ev.load_evidence(args.repo, args.suite, args.record)
    artifact = ev.read_json(args.study)
    rows = validate_artifact(
        artifact, record, hashlib.sha256(args.record.read_bytes()).hexdigest(), pack
    )
    report = compose(suite, rows)
    report["inputs"] = {
        name: {"path": str(path.resolve()), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        for name, path in (("suite", args.suite), ("record", args.record), ("study", args.study))
    }
    report["source"] = {
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": pack["file_universe_digest"],
        "query_pack_sha256": record["query_pack_sha256"],
    }
    require(
        not args.out.resolve().is_relative_to(Path(__file__).resolve().parents[3])
        and not args.out.resolve().is_relative_to(args.repo.resolve()),
        "output must be outside the code and corpus checkouts",
    )
    with args.out.open("x", encoding="utf-8") as handle:
        handle.write(json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
