"""Independent file-prefix and archive-custody checks for the five-product diagnostic."""

from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

import pytest

from tools.benchmark.retrieval.evaluator import canonical, digest
from tools.benchmark.retrieval.lexical_five_product_oracle import (
    _evidence_archive,
    score_record,
)


def record_fixture():
    query_sha = hashlib.sha256(b"Needle").hexdigest()
    pack = {"tasks": [{"task_id": "S01", "query": "Needle", "query_sha256": query_sha}]}
    record = {
        "query_pack_sha256": digest(canonical(pack)),
        "results": [
            {
                "task_id": "S01",
                "route": "lexical",
                "status": "capped",
                "error": None,
                "query_identity": {"original_query_sha256": query_sha},
                "candidates": [
                    {"rank": 1, "path": "src/a.go", "file_sha256": "a" * 64},
                    {"rank": 2, "path": "src/a.go", "file_sha256": "a" * 64},
                    {"rank": 3, "path": "src/b.go", "file_sha256": "b" * 64},
                ],
            }
        ],
    }
    original_gold = {"S01": {"src/a.go"}}
    scoring_gold = {"S01": {"src/a.go", "src/b.go"}}
    universe = {"src/a.go": "a" * 64, "src/b.go": "b" * 64}
    report = {"S01": {"status": "capped", "candidates": 3, "file_hit_at_10": True}}
    return record, pack, original_gold, scoring_gold, universe, report


def test_native_chunk_prefix_deduplicates_files_before_file_ndcg():
    record, pack, original, judged, universe, report = record_fixture()
    result = score_record(record, pack, pack, original, judged, universe, "lexical", report)
    assert result["file_hit_in_chunk_prefix"] == 1
    assert result["file_recall_in_chunk_prefix"] == 1.0
    assert result["observed_prefix_file_ndcg"] == 1.0
    assert result["mean_distinct_files_in_10_chunks"] == 2.0
    assert result["ten_distinct_files_in_10_chunks"] == 0
    assert result["capped_tasks"] == 1
    assert result["per_query"][0]["paths_in_chunk_prefix"] == [
        "src/a.go",
        "src/a.go",
        "src/b.go",
    ]


@pytest.mark.parametrize("mutation", ["query", "rank", "hash", "report", "status"])
def test_native_record_refuses_tampered_query_candidate_or_report(mutation):
    record, pack, original, judged, universe, report = record_fixture()
    record, pack, report = copy.deepcopy((record, pack, report))
    if mutation == "query":
        record["results"][0]["query_identity"]["original_query_sha256"] = "0" * 64
    elif mutation == "rank":
        record["results"][0]["candidates"][0]["rank"] = True
    elif mutation == "hash":
        record["results"][0]["candidates"][0]["file_sha256"] = "0" * 64
    elif mutation == "report":
        report["S01"]["file_hit_at_10"] = False
    else:
        record["results"][0]["status"] = "failed"
    with pytest.raises(ValueError):
        score_record(record, pack, pack, original, judged, universe, "lexical", report)


def test_native_archive_requires_matching_evidence_and_frozen_inputs(tmp_path: Path):
    archive = tmp_path / "native-tree.zip"
    archive.write_bytes(b"retained archive")
    suite, pack = b"suite", b"pack"
    evidence = {
        "family": "retrieval-pair",
        "case_id": "fixed_window_strict.lexical.context",
        "source": {"dirty": False, "revision": "a" * 40},
        "command": {"status": "completed", "exit_code": 0},
        "verdict": {"metrics": [], "reason": None, "scope": "diagnostic", "status": "pass"},
        "inputs": [
            {"id": "suite", "digest": "sha256:" + digest(suite)},
            {"id": "query_pack", "digest": "sha256:" + digest(pack)},
        ],
        "raw": [
            {
                "path": "raw/native-tree.zip",
                "sha256": "sha256:" + digest(archive.read_bytes()),
                "bytes": archive.stat().st_size,
            }
        ],
    }
    evidence_path = tmp_path / "evidence.json"
    evidence_path.write_text(json.dumps(evidence))
    assert _evidence_archive(evidence_path, archive, suite, pack)["source_revision"] == "a" * 40
    archive.write_bytes(b"tampered archive")
    with pytest.raises(ValueError, match="digest or byte count differs"):
        _evidence_archive(evidence_path, archive, suite, pack)
    archive.write_bytes(b"retained archive")
    with pytest.raises(ValueError, match="does not bind original suite and pack"):
        _evidence_archive(evidence_path, archive, suite, b"different pack")
