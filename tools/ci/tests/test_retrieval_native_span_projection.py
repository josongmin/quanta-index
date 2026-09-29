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


def symbol_inputs():
    # Two distinct declarations on one line share the returned context span.
    # Published IDs and indexed spans, rather than context size, distinguish them.
    scored = [
        {
            "path": "src/same.go",
            "start_byte": 0,
            "end_byte": 80,
            "span_accounting": {
                "unit_id": unit,
                "unit_kind": "symbol",
                "indexed_start_byte": start,
                "indexed_end_byte": end,
            },
        }
        for unit, start, end in [("left", 10, 25), ("right", 40, 58)]
    ]
    candidates = [{"candidate_id": "left"}, {"candidate_id": "right"}]
    projection = {
        "policy": "symbol-unit-v1",
        "hits": [
            {
                "candidate_id": unit,
                "path": "src/same.go",
                "start_byte": 0,
                "end_byte": 80,
                "scored_rank": rank,
            }
            for rank, unit in [(1, "left"), (2, "right")]
        ],
    }
    return projection, candidates, scored


def test_symbol_projection_preserves_distinct_same_line_declarations():
    projection, candidates, scored = symbol_inputs()
    run._validate_native_span_projection(
        projection, candidates, scored, 2, "fixture", rank_unit="symbol"
    )
    with pytest.raises(run.RunError, match="native projection"):
        run._validate_native_span_projection(projection, candidates, scored, 2, "fixture")


@pytest.mark.parametrize(
    "mutation", ["policy", "unit", "kind", "rank", "span", "order", "first", "missing"]
)
def test_symbol_projection_refuses_identity_rank_span_and_count_tampering(mutation):
    projection, candidates, scored = symbol_inputs()
    if mutation == "policy":
        projection["policy"] = "first-source-span-v1"
    elif mutation == "unit":
        projection["hits"][1]["candidate_id"] = "foreign"
    elif mutation == "kind":
        scored[1]["span_accounting"]["unit_kind"] = "chunk"
    elif mutation == "rank":
        projection["hits"][1]["scored_rank"] = 1
    elif mutation == "span":
        projection["hits"][1]["end_byte"] = 79
    elif mutation == "order":
        projection["hits"].reverse()
    elif mutation == "first":
        candidates[0]["candidate_id"] = "substituted"
    elif mutation == "missing":
        projection["hits"].pop()
    with pytest.raises(run.RunError, match="native projection"):
        run._validate_native_span_projection(
            projection, candidates, scored, 2, "fixture", rank_unit="symbol"
        )


def test_symbol_projection_requires_bound_symbol_route_and_rank_unit(monkeypatch):
    projection, candidates, scored = symbol_inputs()
    monkeypatch.setattr(run, "_typed_window", lambda *_args: (2, True, {}))
    monkeypatch.setattr(run, "_validate_explanation", lambda *_args: None)
    row = {
        "response_kind": "returned_window",
        "response": {"window": {}, "explanation": {}, "native_projection": projection},
        "candidates": candidates,
        "status": "success",
        "error_code": None,
    }
    reference = {"candidates": scored, "rank_unit": "symbol"}
    run._validate_diagnostic_response_v3(row, ("T1", "symbol"), 6, "enabled", reference, 10)
    with pytest.raises(run.RunError, match="symbol projection"):
        run._validate_diagnostic_response_v3(row, ("T1", "lexical"), 6, "enabled", reference, 10)
    with pytest.raises(run.RunError, match="native projection"):
        run._validate_diagnostic_response_v3(
            row, ("T1", "symbol"), 6, "enabled", {"candidates": scored}, 10
        )
