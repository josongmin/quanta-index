"""Fail-closed checks for the exploratory code-search lexical diagnostic."""

from __future__ import annotations

import hashlib
import json

import pytest

from tools.benchmark.retrieval.evaluator import canonical, digest
from tools.benchmark.retrieval.lexical_file_comparison import pair_result, product_result
from tools.benchmark.retrieval.prepare_lexical_pair import build_spec


def fixture_inputs(tmp_path):
    binary = tmp_path / "searchd"
    binary.write_bytes(b"pinned binary")
    suite_tasks = []
    original_tasks = []
    pack_tasks = []
    for number in range(20):
        task_id = f"S{number:02d}"
        query = f"symbol_{number}"
        sha = hashlib.sha256(query.encode()).hexdigest()
        common = {"task_id": task_id, "gold": [{"path": f"src/{number}.go"}]}
        suite_tasks.append({**common, "query": query, "query_sha256": sha})
        original_query = f"Find function named {query}."
        original_tasks.append(
            {
                "task_id": task_id,
                "gold": [{"path": f"src/{number}.go"}],
                "query": original_query,
                "query_sha256": hashlib.sha256(original_query.encode()).hexdigest(),
            }
        )
        pack_tasks.append({"task_id": task_id, "query": query, "query_sha256": sha})
    common_suite = {
        "repository_commit": "a" * 40,
        "file_universe_digest": "b" * 64,
        "file_universe": [],
        "comparison_contract": {"top_k": 10},
        "routes": ["lexical", "semble-hybrid"],
    }
    suite = {**common_suite, "tasks": suite_tasks}
    original = {**common_suite, "tasks": original_tasks}
    pack = {
        **common_suite,
        "tasks": pack_tasks,
        "suite_commitment_sha256": digest(canonical(suite)),
    }
    base = {
        "scope": "exploratory",
        "claims": {"quality": False},
        "searchd_binary": str(binary),
        "searchd_expected_sha256": "0" * 64,
    }
    return base, original, suite, pack


def test_prepare_binds_current_binary_and_pure_lexical_profiles(tmp_path):
    base, original, suite, pack = fixture_inputs(tmp_path)
    spec = build_spec(
        base,
        original,
        suite,
        pack,
        suite_path=tmp_path / "suite.json",
        pack_path=tmp_path / "pack.json",
        output_root=tmp_path / "capture",
        run_id="lexical-test",
    )
    assert spec["searchd_expected_sha256"] == hashlib.sha256(b"pinned binary").hexdigest()
    assert spec["execution_profiles"]["quanta"]["policy"] == "native"
    assert spec["execution_profiles"]["semble"]["mode"] == "lexical-only"
    assert spec["routes"] == ["lexical"]


def test_prepare_rejects_changed_gold_and_missing_task(tmp_path):
    base, original, suite, pack = fixture_inputs(tmp_path)
    suite["tasks"][0]["gold"][0]["path"] = "wrong.go"
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    with pytest.raises(ValueError, match="changed more than its query"):
        build_spec(
            base,
            original,
            suite,
            pack,
            suite_path=tmp_path / "suite.json",
            pack_path=tmp_path / "pack.json",
            output_root=tmp_path / "capture",
            run_id="lexical-test",
        )


def test_product_result_rejects_wrong_query_and_duplicate(tmp_path):
    expected = {
        f"S{number:02d}": (f"symbol_{number}", [f"src/{number}.go"]) for number in range(20)
    }
    rows = [
        {
            "lane": "symbol_only",
            "task_id": task_id,
            "submitted_query": query,
            "gold_paths": gold,
            "http_status": 200,
            "error": None,
            "file_paths_top_10": gold,
            "file_hit_at_10": True,
        }
        for task_id, (query, gold) in expected.items()
    ]
    path = tmp_path / "rows.jsonl"
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8")
    assert product_result("sourcegraph", path, expected)["hits"] == 20
    rows[0]["submitted_query"] = "wrong"
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8")
    with pytest.raises(ValueError, match="query or gold differs"):
        product_result("sourcegraph", path, expected)
    rows[0]["submitted_query"] = expected[rows[0]["task_id"]][0]
    rows[1]["task_id"] = rows[0]["task_id"]
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8")
    with pytest.raises(ValueError, match="missing or duplicate task"):
        product_result("sourcegraph", path, expected)


def test_pair_result_rejects_semantic_lane_even_if_report_has_hits(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    report = {
        "query_pack_sha256": digest(canonical(pack)),
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "sample_count": 20,
        "rank_metrics": {
            "routes": {
                route: {"sample_count": 20, "chunk": {"file_recall_at_10": 1.0}}
                for route in ("lexical", "semble-hybrid")
            }
        },
    }
    lock = {
        "execution_profiles": {
            "quanta": {
                "profile_id": "quanta-native-v1",
                "policy": "native",
                "config": {},
                "planning_cost_in_latency": False,
            },
            "semble": {
                "profile_id": "semble-lexical-only-v1",
                "mode": "lexical-only",
                "alpha": None,
                "rerank": "not_applicable",
            },
        }
    }
    native = {
        "semble_profile": "lexical-only",
        "rerank_applied": False,
        "lane_call_counts": {"bm25": 1, "semantic": 0, "encode": 0},
        "execution_events": [{"lane_entry_counts": {"bm25": 1, "semantic": 0}}],
    }
    verdict = {"states": {"PAIR_VALID": "pass"}}
    paths = [tmp_path / f"{name}.json" for name in ("report", "lock", "native", "verdict")]
    for path, value in zip(paths, (report, lock, native, verdict), strict=True):
        path.write_text(json.dumps(value), encoding="utf-8")
    assert pair_result(*paths, pack, suite, 20)["routes"]["quanta_lexical"]["hits"] == 20
    native["lane_call_counts"]["semantic"] = 1
    paths[2].write_text(json.dumps(native), encoding="utf-8")
    with pytest.raises(ValueError, match="did not execute lexical-only"):
        pair_result(*paths, pack, suite, 20)
