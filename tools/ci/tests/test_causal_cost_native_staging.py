"""Fixed Native harness stderr markers are progress evidence, not product proof."""

from __future__ import annotations

import json

import pytest

from tools.benchmark.retrieval.causal_cost_diagnostic import main
from tools.benchmark.retrieval.causal_cost_native_staging import build_native_staging_report


def _begin(page: int, pages: int, sources: int, elapsed: int) -> str:
    return json.dumps(
        {
            "event": "native_stage_sources_page",
            "state": "begin",
            "page": page,
            "total_pages": pages,
            "sources": sources,
            "stage_elapsed_ms": elapsed,
        },
        separators=(",", ":"),
    )


def _ack(page: int, pages: int, accepted: int, elapsed: int, duration: int) -> str:
    return json.dumps(
        {
            "event": "native_stage_sources_page",
            "state": "ack",
            "page": page,
            "total_pages": pages,
            "accepted_total": accepted,
            "stage_elapsed_ms": elapsed,
            "page_elapsed_ms": duration,
        },
        separators=(",", ":"),
    )


def _log(*lines: str) -> str:
    return "\n".join(lines) + "\n"


def test_complete_two_page_report_ignores_unrelated_lines_without_claiming_product() -> None:
    report = build_native_staging_report(
        _log(
            "daemon startup",
            _begin(1, 2, 128, 0),
            _ack(1, 2, 128, 12, 12),
            '{"event":"another_event","state":"ack"}',
            _begin(2, 2, 3, 13),
            _ack(2, 2, 131, 50, 37),
        )
    )
    assert report == {
        "status": "COMPLETE",
        "total_pages": 2,
        "begun_pages": 2,
        "acked_pages": 2,
        "last_accepted_count": 131,
        "open_page": None,
        "slowest_ack": {"page": 2, "page_elapsed_ms": 37},
    }
    assert "product_status" not in report


def test_no_marker_and_incomplete_prefixes_are_distinct() -> None:
    assert build_native_staging_report("startup\n") == {
        "status": "NO_MARKERS",
        "total_pages": None,
        "begun_pages": 0,
        "acked_pages": 0,
        "last_accepted_count": None,
        "open_page": None,
        "slowest_ack": None,
    }
    report = build_native_staging_report(
        _log(_begin(1, 2, 128, 1), _ack(1, 2, 128, 8, 7), _begin(2, 2, 5, 9))
    )
    assert report["status"] == "INCOMPLETE"
    assert report["last_accepted_count"] == 128
    assert report["open_page"] == 2
    assert report["slowest_ack"] == {"page": 1, "page_elapsed_ms": 7}
    assert (
        build_native_staging_report(_log(_begin(1, 2, 128, 1), _ack(1, 2, 128, 8, 7)))["status"]
        == "INCOMPLETE"
    )
    assert build_native_staging_report(_log(_begin(1, 256, 128, 0)))["total_pages"] == 256


@pytest.mark.parametrize(
    "raw",
    [
        _log(_ack(1, 1, 1, 2, 1)),
        _log(_begin(1, 1, 1, 0), _begin(1, 1, 1, 1)),
        _log(_begin(1, 2, 128, 0), _ack(2, 2, 128, 3, 3)),
        _log(_begin(1, 2, 128, 0), _ack(1, 3, 128, 3, 3)),
        _log(_begin(1, 2, 127, 0)),
        _log(_begin(1, 1, 1, 4), _ack(1, 1, 2, 5, 1)),
        _log(_begin(1, 1, 1, 4), _ack(1, 1, 1, 5, 2)),
        _log(_begin(1, 1, 1, 4), _ack(1, 1, 1, 3, 0)),
        _log(_begin(1, 257, 128, 0)),
        _log(_begin(1, 1, 0, 0)),
    ],
)
def test_rejects_broken_order_counts_and_clocks(raw: str) -> None:
    with pytest.raises(ValueError):
        build_native_staging_report(raw)


@pytest.mark.parametrize(
    "raw",
    [
        _log('{"event":"native_stage_sources_page","state":"begin"'),
        _log('{"event":"native_stage_sources_page","event":"native_stage_sources_page"}'),
        _log(_begin(True, 1, 1, 0)),
        _log(_begin(1, 1, 1, float("nan"))),
        _log(_begin(1, 1, 1, -1)),
        _log(_begin(1, 1, 1, 0)[:-1] + ',"path":"secret"}'),
    ],
)
def test_rejects_malformed_or_extra_marker_data(raw: str) -> None:
    with pytest.raises(ValueError):
        build_native_staging_report(raw)


def test_cli_keeps_unacknowledged_page_visible(tmp_path, capsys) -> None:
    log = tmp_path / "stderr"
    log.write_text(_log(_begin(1, 1, 16, 0)), encoding="utf-8")
    assert main(["native-staging", "--input", str(log)]) == 0
    report = json.loads(capsys.readouterr().out)
    assert report["status"] == "INCOMPLETE"
    assert report["last_accepted_count"] == 0
    assert report["open_page"] == 1


def test_interrupted_final_marker_preserves_last_complete_ack() -> None:
    report = build_native_staging_report(
        _log(_begin(1, 2, 128, 0), _ack(1, 2, 128, 5, 5), _begin(2, 2, 4, 6))
        + '{"event":"native_stage_sources_page","state":"ack"'
    )
    assert report["status"] == "INCOMPLETE"
    assert report["truncated_tail"] is True
    assert report["last_accepted_count"] == 128
    assert report["open_page"] == 2
