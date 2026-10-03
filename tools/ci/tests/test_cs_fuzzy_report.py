"""Native cs fuzzy scoring separates its same-length matcher by edit class."""

import json
import math

from tools.benchmark.retrieval import cs_fuzzy_report


def test_native_edit_class_is_independent_of_fuzzy_result():
    classify = cs_fuzzy_report.edit_operation
    assert classify("rendar", "render") == "substitution"
    assert classify("rende", "render") == "deletion"
    assert classify("renderr", "render") == "insertion"
    assert classify("redner", "render") == "transposition"
    assert classify("render", "render") == "unchanged"
    assert classify("rɛnder", "render") == "non_ascii"
    assert classify("Rendar", "render") == "substitution"
    assert classify("rrndar", "render") == "other"


def test_native_report_excludes_non_substitution_even_if_raw_result_hits(tmp_path, monkeypatch):
    monkeypatch.setattr(
        cs_fuzzy_report.live_lexical_external,
        "verify_cs_fuzzy",
        lambda _root: {"capability": "cs_fuzzy_osa1_file"},
    )
    tasks = [
        {
            "task_id": task_id,
            "query": query,
            "intended_name": "render",
            "file_judgments": [{"path": "target.go", "grade": 3}],
        }
        for task_id, query in (
            ("sub", "rendar"),
            ("insert", "renderr"),
            ("delete", "rende"),
            ("swap", "redner"),
        )
    ]
    (tmp_path / "suite.json").write_text(json.dumps({"tasks": tasks}))
    (tmp_path / "capture.json").write_text("{}")
    (tmp_path / "cs_fuzzy_rows.jsonl").write_text(
        "\n".join(
            json.dumps(row)
            for row in (
                {"task_id": "sub", "paths": ["wrong.go", "target.go"]},
                {"task_id": "insert", "paths": ["target.go"]},
                {"task_id": "delete", "paths": ["target.go"]},
                {"task_id": "swap", "paths": ["target.go"]},
            )
        )
        + "\n"
    )
    report = cs_fuzzy_report.score_capture(tmp_path)
    assert report["submitted"] == 4
    assert report["compatible"] == 1
    assert (
        report["compatible_edit_contract"]
        == "ascii_casefold_single_substitution_same_length_window"
    )
    assert report["unsupported_task_ids"] == ["delete", "insert", "swap"]
    assert report["raw_hit_at_10_count"] == 4
    assert report["compatible_hit_at_10_count"] == 1
    assert report["compatible_mrr_at_10"] == 0.5
    assert math.isclose(report["compatible_ndcg_at_10"], 1 / math.log2(3))
    assert report["native_by_edit_operation"]["insertion"]["hit_at_10_count"] == 1
    assert report["native_by_edit_operation"]["deletion"]["submitted"] == 1
