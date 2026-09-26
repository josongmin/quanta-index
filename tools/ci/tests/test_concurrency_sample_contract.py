"""Producer defaults and the authoritative concurrency row floor agree."""

from __future__ import annotations

import importlib.util
import re
from pathlib import Path

import pytest

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib

REPO = Path(__file__).resolve().parents[3]
SOURCE = REPO / "crates/quanta-index-searchd-harness/src/concurrency.rs"


def _fixture_module():
    path = Path(__file__).with_name("test_check_bench_artifacts.py")
    spec = importlib.util.spec_from_file_location("concurrency_gate_fixture", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _producer_constant(name: str) -> int:
    match = re.search(rf"pub const {name}: u32 = ([0-9]+);", SOURCE.read_text())
    assert match, f"missing producer contract constant {name}"
    return int(match.group(1))


def test_default_request_budget_and_front_door_match_authoritative_floor() -> None:
    registry = tomllib.loads((REPO / "tools/benchmark/registry.toml").read_text())
    floor = registry["families"]["concurrency"]["sample_floor"]
    assert _producer_constant("MINIMUM_ROW_SAMPLES") == floor
    requests = _producer_constant("DEFAULT_REQUESTS_PER_CLIENT")
    # Public route inventory, with one independently counted reference rotation.
    counts = [sum(index % 5 == route for index in range(requests)) for route in range(5)]
    assert all(count >= floor for count in counts), counts
    recipe = re.search(
        r'^rust-verify-quality-concurrency requests="([0-9]+)":$',
        (REPO / "Justfile").read_text(),
        re.MULTILINE,
    )
    assert recipe and int(recipe.group(1)) == requests
    binary = (SOURCE.parent / "bin/concurrency_matrix.rs").read_text()
    assert "const DEFAULT_REQUESTS_PER_CLIENT:" not in binary
    assert "let mut requests_per_client = DEFAULT_REQUESTS_PER_CLIENT;" in binary


@pytest.mark.parametrize("clients", [1, 8, 32])
def test_complete_route_and_slow_rows_meet_gate_but_underfilled_row_is_rejected(
    clients: int,
) -> None:
    fixture = _fixture_module()
    payload = fixture.artifact("concurrency", clients=clients)
    routes = ["lexical", "semantic", "hybrid", "symbol", "lexical_count", "fast"]
    if clients > 1:
        routes.append("slow")
    assert [row["scenario_id"] for row in payload["rows"]] == [
        f"concurrency.c{clients}.{route}" for route in routes
    ]
    assert payload["concurrency"] == clients + int(clients > 1)
    for row, route in zip(payload["rows"], routes, strict=True):
        expected = 16 if route == "slow" else clients * (80 if route == "fast" else 16)
        assert row["latency"]["samples"] == expected
    reasons = fixture.MODULE.check_artifact(
        payload, dimension="concurrency", head=fixture.HEAD, require=True
    )
    assert reasons == [], reasons
    corrupted = fixture.artifact("concurrency", clients=clients)
    measurement = next(m for m in corrupted["detail"]["measurements"] if m["clients"] == clients)
    measurement["fast"]["requests"] += 1
    reasons = fixture.MODULE.check_artifact(
        corrupted, dimension="concurrency", head=fixture.HEAD, require=True
    )
    assert any("request accounting mismatch" in reason for reason in reasons), reasons
    payload["rows"][-1]["latency"]["samples"] = 15
    reasons = fixture.MODULE.check_artifact(
        payload, dimension="concurrency", head=fixture.HEAD, require=True
    )
    assert any("at least 16 samples" in reason for reason in reasons), reasons
