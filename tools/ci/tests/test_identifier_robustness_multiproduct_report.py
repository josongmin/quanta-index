"""Fixed contracts for the offline five-product typo-strata report."""

import hashlib

import pytest

from tools.benchmark.retrieval import identifier_robustness_multiproduct_report as report


def _task(query="Typ", intended="Type"):
    return {
        "task_id": "fixture.osa.001",
        "query": query,
        "query_sha256": hashlib.sha256(query.encode()).hexdigest(),
        "intended_name": intended,
        "evaluation_contract": {
            "request_mode": "default_file_search",
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        },
        "source_oracle": {"contract": "declaration_name_exact", "unit": "distinct_file"},
        "file_judgments": [
            {"path": "a.go", "grade": 3},
            {"path": "b.go", "grade": 1},
        ],
        "gold": [{"path": "a.go", "grade": 3}],
    }


def test_source_strata_and_evaluator_score_have_independent_fixed_expected_values():
    task = _task()
    assert report._task_source_strata(task) == {
        "literal_relation": "query_proper_substring",
        "surviving_components": "none",
    }
    row = report._result(task, ["b.go", "a.go"], eligible=True, status="capped", latency_ms=4)
    assert row["intended_name_file"]["hit_at_10"] == 1.0
    assert row["intended_name_file"]["mrr_at_10"] == 1.0
    assert row["label_contract"]["intended_name_file"] == "exact_original_name_declaration_files"
    assert "near_name_file" not in row
    assert row["intended_original_file"]["hit_at_10"] == 1.0
    assert row["intended_original_file"]["mrr_at_10"] == 0.5
    assert row["top10_paths"] == ["b.go", "a.go"]
    same_name = report._result(task, ["b.go"], eligible=True, status="success", latency_ms=1)
    assert same_name["intended_name_file"]["hit_at_10"] == 1.0
    assert same_name["intended_original_file"]["hit_at_10"] == 0.0
    for authority in (
        {},
        {"contract": "declaration_name_osa1_casefold", "unit": "distinct_file"},
        {"contract": "declaration_name_exact", "unit": "symbol"},
    ):
        with pytest.raises(report.OfflineReportError, match="exact declaration source authority"):
            report._result(
                {**task, "source_oracle": authority},
                [],
                eligible=True,
                status="success",
                latency_ms=1,
            )


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("query_sha256", "0" * 64),
        ("evaluation_contract", {"gold_unit": "symbol"}),
        ("intended_name", "Other"),
    ],
)
def test_source_task_refuses_wrong_query_gold_or_unit(field, value):
    task = _task()
    task[field] = value
    with pytest.raises(report.OfflineReportError, match="invalid source-bound typo task"):
        report._task_source_strata(task)


def test_top10_rejects_duplicates_out_of_universe_and_unrecorded_extra_rank():
    assert report._top10(["a.go", "b.go"], {"a.go", "b.go"}, "fixture") == ["a.go", "b.go"]
    for paths in (["a.go", "a.go"], ["a.go", "missing.go"], ["a.go"] * 11):
        with pytest.raises(report.OfflineReportError, match="invalid top-10"):
            report._top10(paths, {"a.go", "b.go"}, "fixture")


def test_failed_execution_does_not_score_partial_paths_and_capped_is_eligible():
    task = _task()
    capped = report._result(task, ["a.go"], eligible=True, status="capped", latency_ms=2)
    failed = report._result(task, [], eligible=False, status="error", latency_ms=3)
    summary = report.summarize([capped, failed])
    assert summary["selected"] == 2
    assert summary["eligible"] == 1
    assert summary["intended_original_file"]["hit_count"] == 1
    assert summary["intended_original_file"]["operational"]["hit_at_10"] == 0.5
    assert summary["intended_original_file"]["conditional"]["hit_at_10"] == 1.0
    assert summary["status_counts"] == {"capped": 1, "error": 1}
    assert summary["timing_ms"] == {"count": 2, "sum": 5, "p50": 2.5, "p95": 2.95}
    with pytest.raises(report.OfflineReportError, match="partial top-10"):
        report._result(task, ["a.go"], eligible=False, status="error", latency_ms=3)
