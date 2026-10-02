"""Native cs fuzzy scoring keeps incompatible transpositions out of paired scores."""

import json
import math

from tools.benchmark.retrieval import cs_fuzzy_report


def test_native_one_edit_compatibility_is_distinct_from_osa_transposition():
    assert cs_fuzzy_report.levenshtein_one("rendar", "render")
    assert cs_fuzzy_report.levenshtein_one("rende", "render")
    assert cs_fuzzy_report.levenshtein_one("renderr", "render")
    assert not cs_fuzzy_report.levenshtein_one("redner", "render")
    assert not cs_fuzzy_report.levenshtein_one("render", "render")


def test_native_report_excludes_transposition_even_if_raw_result_hits(tmp_path, monkeypatch):
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
        for task_id, query in (("sub", "rendar"), ("swap", "redner"))
    ]
    (tmp_path / "suite.json").write_text(json.dumps({"tasks": tasks}))
    (tmp_path / "capture.json").write_text("{}")
    (tmp_path / "cs_fuzzy_rows.jsonl").write_text(
        "\n".join(
            json.dumps(row)
            for row in (
                {"task_id": "sub", "paths": ["wrong.go", "target.go"]},
                {"task_id": "swap", "paths": ["target.go"]},
            )
        )
        + "\n"
    )
    report = cs_fuzzy_report.score_capture(tmp_path)
    assert report["submitted"] == 2
    assert report["compatible"] == 1
    assert report["unsupported_task_ids"] == ["swap"]
    assert report["raw_hit_at_10_count"] == 2
    assert report["compatible_hit_at_10_count"] == 1
    assert report["compatible_mrr_at_10"] == 0.5
    assert math.isclose(report["compatible_ndcg_at_10"], 1 / math.log2(3))
