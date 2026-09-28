"""Failure timings cannot count as successful retrieval observations."""

import pytest

from tools.benchmark.retrieval import run


@pytest.mark.parametrize("system", ["quanta", "semble"])
@pytest.mark.parametrize("status", ["error", "timeout", "unavailable"])
def test_failed_task_repetitions_cannot_inflate_floor_or_lower_p95(system, status):
    cell = {
        "system": system,
        "strategy": "whole_file" if system == "quanta" else "native",
        "rows": [("hybrid", "good", "success", 100.0), ("hybrid", "bad", status, 1.0)],
    }
    repeated = {"good": [100.0, 110.0], "bad": [1.0] * 100}
    if system == "quanta":
        cell["warm_latencies"] = {"hybrid": repeated}
    else:
        cell["native_latencies"] = repeated
        cell["native_route"] = "hybrid"
    matrix = run.aggregate_matrix([cell], 1)
    key = f"{system}:{cell['strategy']}:hybrid"
    assert matrix["samples"] == {f"{key}:good": [100.0, 110.0]}
    assert matrix["floors"][key] == 2
    assert matrix["observations_floor"] == 2
    assert matrix["attempts"][key] == 2
    assert matrix["errors"][key] == 1
    assert matrix["summary"][f"{key}:good"]["p95_ms"] == 109.5


@pytest.mark.parametrize("values", [[1.0, float("nan")], [2.0, 3.0]])
def test_failed_task_warm_timings_still_require_valid_bound_values(values):
    cell = {
        "system": "quanta",
        "strategy": "whole_file",
        "rows": [("hybrid", "bad", "error", 1.0)],
        "warm_latencies": {"hybrid": {"bad": values}},
    }
    with pytest.raises(run.RunError):
        run.aggregate_matrix([cell], 1)


def test_all_failed_capture_has_no_observation_floor():
    cell = {
        "system": "quanta",
        "strategy": "whole_file",
        "rows": [("hybrid", "bad", "error", None)],
        "warm_latencies": {"hybrid": {"bad": [1.0] * 1000}},
    }
    matrix = run.aggregate_matrix([cell], 1)
    assert matrix["samples"] == {}
    assert matrix["observations_floor"] == 0
    assert matrix["errors"] == {"quanta:whole_file:hybrid": 1}
