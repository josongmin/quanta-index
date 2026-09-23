"""Independent graph-oracle tests for the SEP-21 product handoff chain."""

from __future__ import annotations

import copy
import importlib.util
import sys
from pathlib import Path


PATH = Path(__file__).resolve().parents[1] / "lint/handoff_validation.py"
spec = importlib.util.spec_from_file_location("quanta_handoff_validation_test", PATH)
assert spec and spec.loader
VALIDATOR = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = VALIDATOR
spec.loader.exec_module(VALIDATOR)


def test_product_lane_order_is_fixed_independent_of_fixture_builder() -> None:
    assert VALIDATOR.PRODUCT_LANES == (
        "P00", "P01", "P02A", "P02B", "P02I", "P03", "P04", "P05",
        "P06", "P07", "P08", "P09", "P10", "P11",
    )
    assert VALIDATOR.HANDOFF_POLICIES["P02I"]["required"] == [
        "p02a-repomap-compiler", "p02b-operation-journal"
    ]


def _chain() -> list[dict]:
    lanes = VALIDATOR.PRODUCT_LANES
    records = [
        {
            "lane": lane,
            "status": "OWNER_PROOF_GREEN",
            "base_sha": f"{index:040x}",
            "result_sha": f"{index + 1:040x}",
        }
        for index, lane in enumerate(lanes)
    ]
    by_lane = {record["lane"]: record for record in records}
    by_lane["P02B"]["base_sha"] = by_lane["P01"]["result_sha"]
    by_lane["P02I"]["base_sha"] = by_lane["P01"]["result_sha"]
    by_lane["P02I"]["integration_commits"] = [
        {"lane": lane, "original_sha": by_lane[lane]["result_sha"]}
        for lane in ("P02A", "P02B")
    ]
    for preceding, following in zip(lanes[5:-1], lanes[6:]):
        by_lane[following]["base_sha"] = by_lane[preceding]["result_sha"]
    return records


def test_product_chain_accepts_exact_fork_join_and_serial_edges() -> None:
    assert VALIDATOR.validate_product_handoff_chain(_chain()) == []


def test_product_chain_refuses_missing_duplicate_and_reordered_lanes() -> None:
    handoffs = _chain()
    for broken in (
        handoffs[1:],
        handoffs[:2] + [handoffs[2]] + handoffs[2:],
        handoffs[:2] + [handoffs[3], handoffs[2]] + handoffs[4:],
    ):
        assert "fixed order" in VALIDATOR.validate_product_handoff_chain(broken)[0]


def test_product_chain_refuses_fork_join_and_immediate_serial_drift() -> None:
    handoffs = _chain()
    for lane, field, value, expected in (
        ("P02B", "base_sha", "f" * 40, "P02B base_sha differs from P01"),
        ("P02I", "base_sha", "f" * 40, "P02I base_sha is not"),
        ("P09", "base_sha", "f" * 40, "P09 base_sha differs from P08"),
    ):
        broken = copy.deepcopy(handoffs)
        next(item for item in broken if item["lane"] == lane)[field] = value
        assert any(expected in error for error in VALIDATOR.validate_product_handoff_chain(broken))

    broken = copy.deepcopy(handoffs)
    join = next(item for item in broken if item["lane"] == "P02I")
    join["integration_commits"][1]["original_sha"] = "f" * 40
    assert any(
        "P02I P02B original_sha differs" in error
        for error in VALIDATOR.validate_product_handoff_chain(broken)
    )


def test_product_chain_refuses_unproven_handoff_and_extra_join_item() -> None:
    broken = _chain()
    next(item for item in broken if item["lane"] == "P07")["status"] = "BLOCKED"
    join = next(item for item in broken if item["lane"] == "P02I")
    join["integration_commits"].append("unvalidated")
    errors = VALIDATOR.validate_product_handoff_chain(broken)
    assert "P07 has no recorded owner-proof handoff" in errors
    assert "P02I integration_commits must name P02A then P02B" in errors
