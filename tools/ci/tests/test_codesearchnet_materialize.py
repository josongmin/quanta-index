"""CodeSearchNet materialization keeps source spans and coverage explicit."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from tools.benchmark.retrieval import codesearchnet_materialize as csn

COMMIT_A = "0123456789abcdef0123456789abcdef01234567"
COMMIT_B = "89abcdef0123456789abcdef0123456789abcdef"


def _url(commit: str, span: str) -> str:
    return f"https://github.com/example/project/blob/{commit}/src/same.py#{span}"


def _qrel(query: str, url: str, mean_grade: float = 2.5) -> dict:
    return {
        "language": "python",
        "query": query,
        "github_url": url,
        "mean_grade": mean_grade,
        "grade_histogram": {"0": 0, "1": 0, "2": 1, "3": 1},
        "annotations": [{"record_index": 1, "grade": 2}, {"record_index": 2, "grade": 3}],
    }


def test_same_source_multiple_spans_and_same_path_other_commit_stay_distinct(tmp_path):
    qrels = [
        _qrel("first", _url(COMMIT_A, "L1-L2")),
        _qrel("second", _url(COMMIT_A, "L2-L3")),
        _qrel("third", _url(COMMIT_B, "L1")),
    ]
    fetched = []

    def fetch(url):
        fetched.append(url)
        return {
            "status": "fetched",
            "http_status": 200,
            "attempts": 1,
            "data": b"one\r\ntwo\r\nthree\r\n",
        }

    root = tmp_path / "corpus"
    manifest = csn.materialize(qrels, root, fetcher=fetch)
    assert len(fetched) == 2
    assert manifest["source_fetch_count"] == 2
    assert manifest["complete_materialization"] is True
    assert manifest["coverage"]["python"] == {
        "qrels_total": 3,
        "qrels_admitted": 3,
        "tasks_total": 3,
        "tasks_complete": 3,
    }
    rows = json.loads((root / "qrels.json").read_text())
    paths = [root / row["snippet_path"] for row in rows]
    assert len({str(path) for path in paths}) == 3
    assert [path.read_bytes() for path in paths] == [
        b"one\r\ntwo\r\n",
        b"two\r\nthree\r\n",
        b"one\r\n",
    ]
    assert [row["mean_grade"] for row in rows] == [2.5, 2.5, 2.5]
    assert manifest["license_status"] == "original_repository_file_licenses_unverified"


def test_missing_source_and_invalid_span_keep_full_task_denominator(tmp_path):
    ok = _url(COMMIT_A, "L1")
    missing = _url(COMMIT_B, "L1")
    invalid = _url(COMMIT_A, "L30")
    qrels = [_qrel("shared task", ok), _qrel("shared task", missing), _qrel("other", invalid)]

    def fetch(url):
        if COMMIT_B in url:
            return {"status": "http_error", "http_status": 404, "attempts": 1}
        return {"status": "fetched", "http_status": 200, "attempts": 1, "data": b"line\n"}

    root = tmp_path / "partial"
    manifest = csn.materialize(qrels, root, fetcher=fetch)
    assert manifest["complete_materialization"] is False
    assert manifest["qrel_count"] == 3
    assert manifest["span_statuses"] == {
        "admitted": 1,
        "invalid_line_span": 1,
        "source_unavailable": 1,
    }
    assert manifest["coverage"]["python"] == {
        "qrels_total": 3,
        "qrels_admitted": 1,
        "tasks_total": 2,
        "tasks_complete": 0,
    }
    rows = json.loads((root / "qrels.json").read_text())
    assert [row["materialization_status"] for row in rows] == [
        "admitted",
        "source_unavailable",
        "invalid_line_span",
    ]
    assert rows[1]["snippet_path"] is None


def test_output_root_rejects_checkout_alias_relative_and_existing(tmp_path, monkeypatch):
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    alias = tmp_path / "checkout-alias"
    alias.symlink_to(checkout, target_is_directory=True)
    monkeypatch.setattr(csn.codesearchnet_qrels, "TOOL_CHECKOUT", checkout.resolve())
    for root in (checkout / "run", alias / "run", Path("relative-run")):
        with pytest.raises(csn.MaterializationError, match="absolute|outside"):
            csn.materialize([], root, fetcher=lambda _url: None)
    existing = tmp_path / "existing"
    existing.mkdir()
    with pytest.raises(csn.MaterializationError, match="must be new"):
        csn.materialize([], existing, fetcher=lambda _url: None)
