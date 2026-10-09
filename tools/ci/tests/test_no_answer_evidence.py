"""Negative retrieval evidence uses successful, judged observations only."""

from __future__ import annotations

import copy

import pytest

from tools.benchmark.retrieval import evaluator as ev


@pytest.mark.parametrize("status", ["timeout", "error", "unavailable"])
def test_failed_negative_has_no_empty_or_nonempty_score(status):
    summary = ev.no_answer_diagnostics(
        {"N": {"answerable": False}},
        {("N", "lexical"): {"status": status, "candidates": []}},
        "lexical",
    )
    assert summary["sample_count"] == 1
    assert summary["eligible_count"] == 0
    assert summary["eligible_task_ids"] == []
    assert summary["excluded"] == [{"task_id": "N", "reason": "execution_status_" + status}]
    assert summary["abstention_rate"] == ev.NOT_APPLICABLE
    assert summary["nonempty_result_rate"] == ev.NOT_APPLICABLE


@pytest.mark.parametrize("policy", [ev.COMPLETE_JUDGMENT_POLICY, ev.SOURCE_ORACLE_JUDGMENT_POLICY])
def test_negative_pool_coverage_is_explicit(policy):
    summary = ev.no_answer_diagnostics(
        {"N": {"answerable": False, "judgment_policy": policy, "file_judgments": []}},
        {
            ("N", "lexical"): {
                "status": "success",
                "candidates": [{"path": "unknown.go", "rank": 1}],
            }
        },
        "lexical",
    )
    if policy == ev.COMPLETE_JUDGMENT_POLICY:
        assert summary["eligible_count"] == 0
        assert summary["excluded"] == [{"task_id": "N", "reason": "unjudged_ranked_file"}]
        assert summary["nonempty_result_rate"] == ev.NOT_APPLICABLE
    else:
        assert summary["eligible_count"] == 1
        assert summary["nonempty_result_rate"] == 1.0


def test_negative_rates_use_successful_judged_denominator():
    tasks = {
        task: {
            "answerable": False,
            "judgment_policy": ev.COMPLETE_JUDGMENT_POLICY,
            "file_judgments": [{"path": "irrelevant.go", "grade": 0}],
        }
        for task in ("empty", "nonempty", "failed", "unjudged")
    }
    results = {
        ("empty", "lexical"): {"status": "abstained", "candidates": []},
        ("nonempty", "lexical"): {
            "status": "success",
            "candidates": [{"path": "irrelevant.go", "rank": 1}],
        },
        ("failed", "lexical"): {"status": "timeout", "candidates": []},
        ("unjudged", "lexical"): {
            "status": "success",
            "candidates": [{"path": "unknown.go", "rank": 1}],
        },
    }
    summary = ev.no_answer_diagnostics(tasks, results, "lexical")
    assert summary["sample_count"] == 4
    assert summary["eligible_count"] == 2
    assert summary["eligible_task_ids"] == ["empty", "nonempty"]
    assert summary["abstention_rate"] == summary["nonempty_result_rate"] == 0.5
    assert summary["status_counts"] == {"abstained": 1, "success": 2, "timeout": 1}


@pytest.fixture
def paired_negative(tmp_path):
    from tools.benchmark.retrieval import semble as semble_adapter
    from tools.ci.tests.test_retrieval_benchmark import _file_projection_run

    repo, suite, run, _sp, _rp = _file_projection_run(tmp_path, "code_search_file")
    suite["routes"] = ["lexical", "semble-lexical-file"]
    for task in suite["tasks"]:
        task["judgment_policy"] = ev.COMPLETE_JUDGMENT_POLICY
    # Source-bound synthetic fixture labels, not independent human relevance.
    negative = suite["tasks"][1]
    negative.update(answerable=False, gold=[], file_judgments=[])
    run["captures"]["s0"] = {
        "system": "semble",
        "execution_profile": semble_adapter.execution_profile("lexical-file", None),
    }
    run["route_provenance"]["semble-lexical-file"] = {"capture_id": "s0"}
    for row in list(run["results"]):
        row["score_evidence"] = "native_sdk_score_v1"
        for rank, candidate in enumerate(row["candidates"]):
            candidate["score"] = float(len(row["candidates"]) - rank)
        baseline = copy.deepcopy(row)
        baseline.update(
            route="semble-lexical-file",
            score_evidence="semble_bm25_score_v1",
            ordering="score_desc_native_tiebreak",
            file_collection={"observed": True},
        )
        run["results"].append(baseline)
    _, pack, _ = ev.validate_suite(repo, suite)
    return suite, pack, run, negative["task_id"]


@pytest.mark.parametrize("route", ["lexical", "semble-lexical-file"])
@pytest.mark.parametrize("status", ["error", "timeout", "unavailable", "success"])
def test_complete_file_boundary_rejects_failed_or_unjudged_negatives(
    paired_negative, route, status
):
    suite, pack, run, negative = paired_negative
    # Start with valid empty controls on both routes, then alter only one route.
    for row in run["results"]:
        if row["task_id"] == negative:
            row.update(status="abstained", candidates=[])
    target = next(
        row for row in run["results"] if (row["task_id"], row["route"]) == (negative, route)
    )
    target.update(
        status=status, candidates=[{"path": "b.txt", "rank": 1}] if status == "success" else []
    )
    for boundary in (ev.complete_scored_file_rows, ev.evaluate_complete_scored_file_evidence):
        with pytest.raises(ev.EvidenceError, match="no-answer"):
            boundary(suite, pack, run, "semble-lexical-file", "lexical")


def test_complete_file_boundary_accepts_judged_nonempty_negatives(paired_negative):
    suite, pack, run, negative = paired_negative
    task = next(task for task in suite["tasks"] if task["task_id"] == negative)
    task["file_judgments"] = [{**row, "grade": 0} for row in suite["file_universe"]]
    rows = ev.complete_scored_file_rows(suite, pack, run, "semble-lexical-file", "lexical")
    assert len(rows) == 1
    report = ev.evaluate_complete_scored_file_evidence(
        suite, pack, run, "semble-lexical-file", "lexical"
    )
    assert report["rank_metrics"]["comparison"]["no_answer_abstention_delta"]["sample_count"] == 1


@pytest.mark.parametrize("negative_empty", [True, False])
def test_common_product_metrics_retain_negative_controls(negative_empty):
    from tools.benchmark.retrieval.lexical_file_comparison import common_product_metrics

    product = {
        "per_query": [
            {
                "task_id": "A",
                "answerable": True,
                "file_hit_at_10": True,
                "file_recall_at_10": 0.5,
                "file_ndcg_at_10": 0.75,
            },
            {"task_id": "N", "answerable": False, "no_gold_empty_at_10": negative_empty},
            {"task_id": "excluded", "answerable": False, "status": "timeout"},
        ]
    }
    assert common_product_metrics(product, {"A", "N"}) == {
        "tasks": 2,
        "answerable_tasks": 1,
        "no_gold_tasks": 1,
        "file_hit_at_10": 1.0,
        "file_recall_at_10": 0.5,
        "file_ndcg_at_10": 0.75,
        "no_gold_empty_rate_at_10": float(negative_empty),
    }
    empty = common_product_metrics(product, set())
    assert empty["tasks"] == empty["no_gold_tasks"] == 0
    assert empty["file_recall_at_10"] == empty["no_gold_empty_rate_at_10"] == ev.NOT_APPLICABLE


def test_common_metrics_reject_missing_or_duplicate_cohort_observations():
    from tools.benchmark.retrieval.lexical_file_comparison import common_product_metrics

    with pytest.raises(ValueError, match="admitted cohort"):
        common_product_metrics({"per_query": []}, {"N"})
    row = {"task_id": "N", "answerable": False, "no_gold_empty_at_10": True}
    with pytest.raises(ValueError, match="admitted cohort"):
        common_product_metrics({"per_query": [row, row]}, {"N"})
