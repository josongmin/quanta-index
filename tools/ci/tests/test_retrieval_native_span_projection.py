"""Native unit cardinality stays separate from scored source-span cardinality."""

import copy

import pytest

from tools.benchmark.retrieval import run


def inputs():
    scored = [
        {"path": "src/a.rs", "start_byte": 0, "end_byte": 10, "span_accounting": {"unit_id": "a"}},
        {"path": "src/a.rs", "start_byte": 20, "end_byte": 30, "span_accounting": {"unit_id": "c"}},
    ]
    candidates = [{"candidate_id": "a"}, {"candidate_id": "c"}]
    projection = {
        "policy": "first-source-span-v1",
        "hits": [
            {
                "candidate_id": "a",
                "path": "src/a.rs",
                "start_byte": 0,
                "end_byte": 10,
                "scored_rank": 1,
            },
            {
                "candidate_id": "b",
                "path": "src/a.rs",
                "start_byte": 0,
                "end_byte": 10,
                "scored_rank": 1,
            },
            {
                "candidate_id": "c",
                "path": "src/a.rs",
                "start_byte": 20,
                "end_byte": 30,
                "scored_rank": 2,
            },
        ],
    }
    return projection, candidates, scored


def test_complete_native_projection_preserves_first_source_span_order():
    projection, candidates, scored = inputs()
    run._validate_native_span_projection(projection, candidates, scored, 3, "fixture")


@pytest.mark.parametrize(
    "mutation", ["count", "unit", "span", "rank", "order", "first", "missing", "policy"]
)
def test_native_projection_refuses_dropped_substituted_or_reordered_hits(mutation):
    projection, candidates, scored = inputs()
    if mutation == "count":
        projection["hits"].pop()
    elif mutation == "unit":
        projection["hits"][1]["candidate_id"] = "a"
    elif mutation == "span":
        projection["hits"][1]["end_byte"] = 11
    elif mutation == "rank":
        projection["hits"][1]["scored_rank"] = True
    elif mutation == "order":
        projection["hits"][0], projection["hits"][2] = projection["hits"][2], projection["hits"][0]
    elif mutation == "first":
        projection["hits"][0]["candidate_id"] = "substituted"
    elif mutation == "missing":
        projection["hits"][2] = {**copy.deepcopy(projection["hits"][0]), "candidate_id": "d"}
    elif mutation == "policy":
        projection["policy"] = "unproved-drop"
    with pytest.raises(run.RunError, match="native projection"):
        run._validate_native_span_projection(projection, candidates, scored, 3, "fixture")


def test_collapsed_window_requires_projection_witness(monkeypatch):
    projection, candidates, scored = inputs()
    monkeypatch.setattr(run, "_typed_window", lambda *_args: (3, True, {}))
    monkeypatch.setattr(run, "_validate_explanation", lambda *_args: None)
    row = {
        "response_kind": "returned_window",
        "response": {"window": {}, "explanation": {}},
        "candidates": candidates,
        "status": "success",
        "error_code": None,
    }
    with pytest.raises(run.RunError, match="returned count differs"):
        run._validate_diagnostic_response_v3(
            row, ("T1", "semantic"), 6, "enabled", {"candidates": scored}, 10
        )
    row["response"]["native_projection"] = projection
    run._validate_diagnostic_response_v3(
        row, ("T1", "semantic"), 6, "enabled", {"candidates": scored}, 10
    )
    with pytest.raises(run.RunError, match="exceeds top_k"):
        run._validate_diagnostic_response_v3(
            row, ("T1", "semantic"), 6, "enabled", {"candidates": scored}, 2
        )
