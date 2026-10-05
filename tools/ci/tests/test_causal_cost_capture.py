"""Frontdoor history policy flags and refusal before creating an output root."""

from __future__ import annotations

import argparse
from pathlib import Path

import pytest

from tools.benchmark.retrieval.causal_cost_capture import _history_inputs, _scale_command, capture


def test_scale_command_forwards_pair_and_total_as_distinct_flags() -> None:
    args = argparse.Namespace(
        tier="xlarge",
        seed=7,
        client_timeout_ms=None,
        history_max_bytes=300_000_000,
        history_max_total_bytes=600_000_000,
    )
    assert _history_inputs(args.history_max_bytes, args.history_max_total_bytes) == {
        "history_policy_id": "explicit-pair-total-diagnostic-v1",
        "requested_history_max_bytes": 300_000_000,
        "history_max_bytes": 300_000_000,
        "requested_history_max_total_bytes": 600_000_000,
        "history_max_total_bytes": 600_000_000,
    }
    assert _scale_command(args, Path("/tmp/scale_matrix"), Path("/tmp/new/artifact")) == [
        "/tmp/scale_matrix",
        "--tier",
        "xlarge",
        "--seed",
        "7",
        "--out-dir",
        "/tmp/new/artifact",
        "--history-max-bytes",
        "300000000",
        "--history-max-total-bytes",
        "600000000",
    ]
    args.history_max_bytes = None
    args.history_max_total_bytes = None
    assert _scale_command(args, Path("/tmp/scale_matrix"), Path("/tmp/new/artifact"))[-2:] == [
        "--out-dir",
        "/tmp/new/artifact",
    ]


@pytest.mark.parametrize(
    ("pair", "total"),
    [
        (None, 600_000_000),
        (600_000_001, 600_000_000),
        (268_435_457, None),
        (0, None),
        (True, None),
        (300_000_000, 0),
        (300_000_000, 600_000_000.0),
        (300_000_000, 1 << 64),
    ],
)
def test_invalid_history_inputs_refuse_without_output(
    pair: object, total: object, tmp_path: Path
) -> None:
    output = tmp_path / "must-remain-absent"
    with pytest.raises(ValueError):
        capture(
            argparse.Namespace(
                history_max_bytes=pair, history_max_total_bytes=total, out_root=output
            )
        )
    assert not output.exists()
