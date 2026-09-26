"""Fail-closed checks for the exploratory code-search lexical diagnostic."""

from __future__ import annotations

import hashlib
import json

import pytest

from tools.benchmark.retrieval.evaluator import canonical, digest
from tools.benchmark.retrieval.lexical_file_comparison import (
    latency_summary,
    pair_result,
    product_result,
)
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
            "elapsed_ms": 1.0,
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


@pytest.mark.parametrize("recall", [1.0, 0.5])
def test_pair_result_rejects_semantic_lane_even_if_report_has_hits(tmp_path, recall):
    _, _, suite, pack = fixture_inputs(tmp_path)
    if recall == 0.5:
        for task in suite["tasks"]:
            task["gold"].append({"path": "src/second-answer.go"})
        pack["suite_commitment_sha256"] = digest(canonical(suite))
    report = {
        "query_pack_sha256": digest(canonical(pack)),
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "sample_count": 20,
        "rank_metrics": {
            "routes": {
                route: {
                    "sample_count": 20,
                    "chunk": {"file_recall_at_10": recall},
                    "mean_query_latency_ms": 1.0,
                }
                for route in ("lexical", "semble-hybrid")
            }
        },
        "per_query": [
            {
                "route": route,
                "task_id": task["task_id"],
                "query_latency_ms": 1.0,
                "file_recall_at_10": recall,
                "file_hit_at_10": True,
            }
            for route in ("lexical", "semble-hybrid")
            for task in pack["tasks"]
        ],
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
    route = pair_result(*paths, pack, suite, 20)["routes"]["quanta_lexical"]
    assert route["file_recall_at_10"] == recall
    assert route["file_hit_rate_at_10"] == 1.0
    report["per_query"][0]["file_recall_at_10"] = 0.0
    paths[0].write_text(json.dumps(report))
    with pytest.raises(ValueError, match="recall/hit observations"):
        pair_result(*paths, pack, suite, 20)
    report["per_query"][0]["file_recall_at_10"] = recall
    paths[0].write_text(json.dumps(report))
    native["lane_call_counts"]["semantic"] = 1
    paths[2].write_text(json.dumps(native), encoding="utf-8")
    with pytest.raises(ValueError, match="did not execute lexical-only"):
        pair_result(*paths, pack, suite, 20)


def test_latency_summary_rejects_missing_and_nonfinite_values():
    values = [float(number) for number in range(1, 21)]
    summary = latency_summary(values, 20, "test-layer")
    assert summary["p50_ms"] == 10.5
    assert summary["p95_ms"] == 19.0
    with pytest.raises(ValueError, match="invalid latency"):
        latency_summary(values[:-1], 20, "test-layer")
    with pytest.raises(ValueError, match="invalid latency"):
        latency_summary(values[:-1] + [float("nan")], 20, "test-layer")


def test_multiple_gold_files_distinguish_hit_rate_from_macro_file_recall(tmp_path):
    expected = {"query": ("symbol", ["first.go", "second.go"])}
    path = tmp_path / "rows.jsonl"
    path.write_text(
        json.dumps(
            {
                "lane": "symbol_only",
                "task_id": "query",
                "submitted_query": "symbol",
                "gold_paths": expected["query"][1],
                "http_status": 200,
                "error": None,
                "file_paths_top_10": ["first.go"],
                "file_hit_at_10": True,
                "elapsed_ms": 1.0,
            }
        )
        + "\n"
    )
    result = product_result("sourcegraph", path, expected)
    assert result["hits"] == 1
    assert result["file_hit_rate_at_10"] == 1.0
    assert result["file_recall_at_10"] == 0.5
    assert result["per_query"][0]["file_recall_at_10"] == 0.5


@pytest.mark.parametrize("raw", [b"[]\n", b'{"task_id":"q","task_id":"forged"}\n', b'{"x":NaN}\n'])
def test_malformed_or_duplicate_raw_rows_refuse(tmp_path, raw):
    path = tmp_path / "rows.jsonl"
    path.write_bytes(raw)
    with pytest.raises(ValueError):
        product_result("sourcegraph", path, {"q": ("symbol", ["answer.go"])})


def test_input_changed_during_read_cannot_bind_new_digest_to_old_score(tmp_path, monkeypatch):
    from pathlib import Path

    from tools.benchmark.retrieval import lexical_file_comparison as lexical

    path = tmp_path / "input.json"
    path.write_bytes(b'{"old":true}')
    original = Path.read_bytes

    def mutate(value):
        data = original(value)
        if value == path:
            path.write_bytes(b'{"new":false}')
        return data

    monkeypatch.setattr(Path, "read_bytes", mutate)
    with pytest.raises(ValueError, match="changed while reading"):
        lexical._read(path)


def test_spec_has_exact_absolute_input_inventory(tmp_path):
    from tools.benchmark.retrieval import lexical_file_comparison as lexical

    path = tmp_path / "spec.json"
    payload = {"schema_version": 1, **{role: str(tmp_path / role) for role in lexical.INPUT_ROLES}}
    path.write_text(json.dumps(payload))
    assert set(lexical.read_spec(path)) == set(lexical.INPUT_ROLES)
    for change in ({"schema_version": True}, {"suite": "relative.json"}, {"unknown": "input"}):
        path.write_text(json.dumps({**payload, **change}))
        with pytest.raises(ValueError):
            lexical.read_spec(path)


def test_owner_cli_refuses_mixed_controls_before_output(tmp_path, monkeypatch):
    import sys

    from tools.benchmark.retrieval import lexical_file_comparison as lexical

    out = tmp_path / "must-not-exist.json"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "lexical",
            "--spec",
            str(tmp_path / "spec.json"),
            "--suite",
            str(tmp_path / "suite.json"),
            "--out",
            str(out),
        ],
    )
    with pytest.raises(SystemExit) as error:
        lexical.main()
    assert error.value.code == 2 and not out.exists()


def test_owner_cli_does_not_overwrite_existing_output(tmp_path, monkeypatch):
    import sys

    from tools.benchmark.retrieval import lexical_file_comparison as lexical

    out = tmp_path / "existing.json"
    out.write_bytes(b"original evidence")
    spec = tmp_path / "spec.json"
    spec.write_text(
        json.dumps(
            {"schema_version": 1, **{role: str(tmp_path / role) for role in lexical.INPUT_ROLES}}
        )
    )
    monkeypatch.setattr(sys, "argv", ["lexical", "--spec", str(spec), "--out", str(out)])
    with pytest.raises(SystemExit) as error:
        lexical.main()
    assert error.value.code == 2 and out.read_bytes() == b"original evidence"
