"""Product result-window wire contracts at the persisted evidence boundary."""

from copy import deepcopy

import pytest

from tools.benchmark.retrieval.result_window import WindowEvidenceError, typed_window


def exact_window(returned: int) -> dict:
    window = {
        "returned": returned,
        "candidate_count": {"kind": "exact", "value": returned},
        "outcome": {"kind": "exact_exhausted"},
        "coverage": {
            "examined": {"kind": "exact", "value": returned},
            "exhaustion_proof": {"kind": "probe_exhausted", "fetched": returned},
            "lanes": [
                {
                    "lane": "lexical",
                    "executed": True,
                    "contributed": returned > 0,
                    "filtered_out": 0,
                    "candidates": {"kind": "exact", "value": returned},
                }
            ],
        },
    }
    if not returned:
        window["empty_provenance"] = "available_empty"
    return window


def test_logical_empty_retains_exhaustion_without_claiming_execution():
    # Public wire shape of QueryResultWindowV2::logical_empty("lexical").
    window = exact_window(0)
    window["empty_provenance"] = "logical_empty"
    window["coverage"]["exhaustion_proof"] = {"kind": "logical_empty"}
    window["coverage"]["lanes"][0]["executed"] = False
    assert typed_window(window, "lexical") == (0, True, {"lexical": False})

    executed = deepcopy(window)
    executed["coverage"]["lanes"][0]["executed"] = True
    with pytest.raises(WindowEvidenceError, match="logical empty"):
        typed_window(executed, "lexical")

    mixed = deepcopy(window)
    mixed["coverage"]["exhaustion_proof"] = {"kind": "probe_exhausted", "fetched": 0}
    with pytest.raises(WindowEvidenceError, match="logical empty"):
        typed_window(mixed, "lexical")


@pytest.mark.parametrize("empty_provenance", [None, "logical_empty", "unknown"])
def test_rows_cannot_carry_empty_provenance(empty_provenance):
    window = exact_window(1)
    window["empty_provenance"] = empty_provenance
    with pytest.raises(WindowEvidenceError, match="empty_provenance"):
        typed_window(window, "lexical")


def test_empty_window_cannot_claim_a_contributing_lane():
    window = exact_window(0)
    window["coverage"]["lanes"][0]["contributed"] = True
    with pytest.raises(WindowEvidenceError, match="contributed"):
        typed_window(window, "lexical")


def test_nonexhausted_window_cannot_carry_a_null_exhaustion_proof():
    window = exact_window(1)
    window["outcome"] = {"kind": "lower_bound", "continuation": False}
    window["coverage"]["exhaustion_proof"] = None
    with pytest.raises(WindowEvidenceError, match="exhaustion proof"):
        typed_window(window, "lexical")


@pytest.mark.parametrize(
    "field,limit",
    [
        ("returned", 2**32 - 1),
        ("cap", 2**32 - 1),
        ("candidate_count", 2**64 - 1),
        ("examined", 2**64 - 1),
        ("candidates", 2**64 - 1),
        ("filtered_out", 2**64 - 1),
        ("cost", 2**64 - 1),
        ("examined_lower_bound", 2**64 - 1),
    ],
)
def test_window_rejects_out_of_range_wire_integer(field, limit):
    window = exact_window(1)
    coverage = window["coverage"]
    lane = coverage["lanes"][0]
    del coverage["exhaustion_proof"]
    window["candidate_count"] = {"kind": "at_least", "value": 2**64 - 1}
    window["outcome"] = {"kind": "capped_unknown", "cap": 1}
    if field == "examined_lower_bound":
        window["outcome"] = {
            "kind": "approximate",
            "method": "ann_search",
            "quality_contract": {"examined_lower_bound": 0},
        }
    owner, key = {
        "returned": (window, "returned"),
        "cap": (window["outcome"], "cap"),
        "candidate_count": (window["candidate_count"], "value"),
        "examined": (coverage["examined"], "value"),
        "candidates": (lane["candidates"], "value"),
        "filtered_out": (lane, "filtered_out"),
        "cost": (lane, "cost"),
        "examined_lower_bound": (window["outcome"].get("quality_contract"), "examined_lower_bound"),
    }[field]
    owner[key] = limit
    typed_window(window, "lexical")
    owner[key] += 1
    with pytest.raises(WindowEvidenceError):
        typed_window(window, "lexical")
