"""Fail-closed checks for the exploratory code-search lexical diagnostic."""

from __future__ import annotations

import hashlib
import json
import math
import subprocess
import sys
from pathlib import Path

import pytest

from tools.benchmark.retrieval.evaluator import canonical, digest
from tools.benchmark.retrieval.lexical_external_oracle import verify_capture_manifest
from tools.benchmark.retrieval.lexical_file_comparison import (
    _file_policy_from_lock,
    _file_universe,
    _tasks,
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
        common = {
            "task_id": task_id,
            "answerable": True,
            "gold": [{"path": f"src/{number}.go"}],
        }
        suite_tasks.append({**common, "query": query, "query_sha256": sha})
        original_query = f"Find function named {query}."
        original_tasks.append(
            {
                "task_id": task_id,
                "answerable": True,
                "gold": [{"path": f"src/{number}.go"}],
                "query": original_query,
                "query_sha256": hashlib.sha256(original_query.encode()).hexdigest(),
            }
        )
        pack_tasks.append({"task_id": task_id, "query": query, "query_sha256": sha})
    common_suite = {
        "schema_version": 3,
        "suite_id": "lexical-fixture-v1",
        "repository_commit": "a" * 40,
        "file_universe_digest": "b" * 64,
        "file_universe": [],
        "comparison_contract": {
            "top_k": 10,
            "tokenizer": "qi-regex-v1",
            "tokenizer_budget_version": "qb-v1",
            "output_unit_policy": "rank_prefix",
            "span_unit": "byte_span_with_line_projection_v1",
        },
        "routes": ["lexical", "semble-lexical-only"],
    }
    suite = {**common_suite, "tasks": suite_tasks}
    original = {**common_suite, "tasks": original_tasks}
    pack = {
        **common_suite,
        "tasks": pack_tasks,
        "tokenizer": "qi-regex-v1",
        "tokenizer_budget_version": "qb-v1",
        "suite_commitment_sha256": digest(canonical(suite)),
    }
    base = {
        "scope": "exploratory",
        "claims": {"quality": False},
        "searchd_binary": str(binary),
        "searchd_expected_sha256": "0" * 64,
    }
    return base, original, suite, pack


def _external_join_fixture(tmp_path):
    from tools.benchmark.retrieval import lexical_file_comparison as owner

    native_roles = [role for role in owner.INPUT_ROLES if not role.endswith("_rows")]
    native = {role: tmp_path / (role + ".json") for role in native_roles}
    for role, path in native.items():
        path.write_bytes((role + " fixed bytes").encode())
    roots = {}
    summaries = {}
    for name in owner.PRODUCTS:
        root = tmp_path / name
        root.mkdir()
        (root / "suite.json").write_bytes(native["suite"].read_bytes())
        (root / "query-pack.json").write_bytes(native["query_pack"].read_bytes())
        (root / "capture.json").write_bytes(b"fixed capture bytes")
        roots[name] = root
        summaries[root] = {
            "schema_version": 2,
            "products": [name],
            "release_digest": "sha256:" + "a" * 64,
            "binding": {"source": "fixed independent fixture"},
            "rows_sha256": {name: "b" * 64},
        }
    return native, roots, summaries


def test_external_join_replays_each_root_before_and_after_existing_scorer(tmp_path, monkeypatch):
    from tools.benchmark.retrieval import lexical_file_comparison as owner
    from tools.benchmark.retrieval import live_lexical_external as live

    native, roots, summaries = _external_join_fixture(tmp_path)
    calls = []

    def replay(root):
        calls.append(root.name)
        return summaries[root]

    def score(paths):
        calls.append("score")
        assert set(paths) == set(owner.INPUT_ROLES)
        for name, root in roots.items():
            assert paths[name + "_rows"] == root / (name + "_rows.jsonl")
        return {
            "status": "diagnostic_unqualified",
            "products": {name: {"raw_sha256": "b" * 64} for name in roots},
        }

    monkeypatch.setattr(live, "verify", replay)
    monkeypatch.setattr(owner, "evaluate_capture", score)
    result = owner.evaluate_external_captures(native, roots)
    assert calls == ["sourcegraph", "opengrok", "cs", "score", "sourcegraph", "opengrok", "cs"]
    assert result["status"] == "diagnostic_unqualified"
    binding = result["external_capture_binding"]
    assert binding["performance_scope"] == "descriptive_only_not_a_paired_speed_experiment"
    assert set(binding["captures"]) == {str(root) for root in roots.values()}


@pytest.mark.parametrize("fault", ["missing_product", "selection", "suite", "pack", "source"])
def test_external_join_refuses_independent_coverage_and_binding_faults(
    tmp_path, monkeypatch, fault
):
    from tools.benchmark.retrieval import lexical_file_comparison as owner
    from tools.benchmark.retrieval import live_lexical_external as live

    native, roots, summaries = _external_join_fixture(tmp_path)
    if fault == "missing_product":
        roots.pop("cs")
    elif fault == "selection":
        summaries[roots["sourcegraph"]]["products"] = ["sourcegraph", "cs"]
    elif fault in {"suite", "pack"}:
        filename = "suite.json" if fault == "suite" else "query-pack.json"
        (roots["sourcegraph"] / filename).write_bytes(b"different bytes")
    else:
        summaries[roots["opengrok"]]["binding"] = {"source": "different source"}
    monkeypatch.setattr(live, "verify", lambda root: summaries[root])

    def must_not_score(paths):
        raise AssertionError("invalid external binding reached the scorer")

    monkeypatch.setattr(owner, "evaluate_capture", must_not_score)
    with pytest.raises(ValueError, match="external"):
        owner.evaluate_external_captures(native, roots)


@pytest.mark.parametrize("fault", ["metadata", "capture_bytes", "native_pack", "scored_rows"])
def test_external_join_refuses_mutation_during_scoring(tmp_path, monkeypatch, fault):
    from tools.benchmark.retrieval import lexical_file_comparison as owner
    from tools.benchmark.retrieval import live_lexical_external as live

    native, roots, summaries = _external_join_fixture(tmp_path)
    monkeypatch.setattr(live, "verify", lambda root: dict(summaries[root]))

    def score(paths):
        if fault == "metadata":
            summaries[roots["cs"]] = {**summaries[roots["cs"]], "tasks": 99}
        elif fault == "capture_bytes":
            (roots["cs"] / "capture.json").write_bytes(b"changed capture bytes")
        elif fault == "native_pack":
            native["query_pack"].write_bytes(b"changed native pack")
        return {
            "status": "diagnostic_unqualified",
            "products": {
                name: {"raw_sha256": ("c" if fault == "scored_rows" and name == "cs" else "b") * 64}
                for name in roots
            },
        }

    monkeypatch.setattr(owner, "evaluate_capture", score)
    with pytest.raises(ValueError, match="changed during|scored external rows differ"):
        owner.evaluate_external_captures(native, roots)


@pytest.mark.parametrize("fault", [None, "unknown", "missing", "relative", "bool_version"])
def test_external_join_spec_has_a_closed_inventory(tmp_path, fault):
    from tools.benchmark.retrieval import lexical_file_comparison as owner

    native, roots, _ = _external_join_fixture(tmp_path)
    value = {
        "schema_version": 1,
        "native_inputs": {role: str(path) for role, path in native.items()},
        "external_captures": {name: str(path) for name, path in roots.items()},
    }
    if fault == "unknown":
        value["allow_partial"] = True
    elif fault == "missing":
        value["external_captures"].pop("cs")
    elif fault == "relative":
        value["native_inputs"]["suite"] = "relative.json"
    elif fault == "bool_version":
        value["schema_version"] = True
    path = tmp_path / "join-spec.json"
    path.write_text(json.dumps(value))
    if fault is None:
        assert owner.read_external_spec(path) == (native, roots)
    else:
        with pytest.raises(ValueError, match="external join"):
            owner.read_external_spec(path)


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
    assert spec["candidate_route"] == "lexical"
    assert spec["baseline_route"] == "semble-lexical-only"
    assert spec["semble_route"] == "semble-lexical-only"


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
    universe = {gold[0] for _, gold in expected.values()}
    assert product_result("sourcegraph", path, expected, universe)["hits"] == 20
    assert product_result("sourcegraph", path, expected, universe)["rank_unit"] == "distinct_file"
    rows[0]["submitted_query"] = "wrong"
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8")
    with pytest.raises(ValueError, match="query or gold differs"):
        product_result("sourcegraph", path, expected, universe)
    rows[0]["submitted_query"] = expected[rows[0]["task_id"]][0]
    rows[1]["task_id"] = rows[0]["task_id"]
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8")
    with pytest.raises(ValueError, match="missing or duplicate task"):
        product_result("sourcegraph", path, expected, universe)


def test_external_oracle_rescore_keeps_capture_gold_and_uses_independent_multifile_gold(tmp_path):
    expected = {"S01": ("Needle", ["src/original.go"])}
    universe = {"src/original.go", "src/other.go", "src/irrelevant.go"}
    row = {
        "lane": "symbol_only",
        "task_id": "S01",
        "submitted_query": "Needle",
        "gold_paths": ["src/original.go"],
        "http_status": 200,
        "error": None,
        "file_paths_top_10": ["src/other.go", "src/irrelevant.go", "src/original.go"],
        "file_hit_at_10": True,
        "elapsed_ms": 1.0,
    }
    path = tmp_path / "rows.jsonl"
    path.write_text(json.dumps(row) + "\n")
    rescored = product_result(
        "sourcegraph",
        path,
        expected,
        universe,
        scoring_gold={"S01": ["src/original.go", "src/other.go"]},
    )
    assert rescored["file_recall_at_10"] == 1.0
    assert rescored["file_ndcg_at_10"] == pytest.approx((1 + 1 / 2) / (1 + 1 / math.log2(3)))
    row["file_hit_at_10"] = False
    path.write_text(json.dumps(row) + "\n")
    with pytest.raises(ValueError, match="hit flag differs"):
        product_result(
            "sourcegraph",
            path,
            expected,
            universe,
            scoring_gold={"S01": ["src/original.go", "src/other.go"]},
        )


@pytest.mark.parametrize("product", ["sourcegraph", "opengrok", "cs"])
def test_external_unjudged_files_cannot_be_silently_scored_as_irrelevant(tmp_path, product):
    expected = {"Q1": ("Needle", ["answer.go"])}
    row = {
        "lane": "symbol_only",
        "task_id": "Q1",
        "submitted_query": "Needle",
        "gold_paths": ["answer.go"],
        "file_hit_at_10": True,
        "elapsed_ms": 1.0,
    }
    if product == "cs":
        row.update(exit_code=0, paths=["unknown.go", "answer.go"])
    else:
        row.update(http_status=200, error=None, file_paths_top_10=["unknown.go", "answer.go"])
        if product == "opengrok":
            row["field"] = "full"
    path = tmp_path / "rows.jsonl"
    path.write_text(json.dumps(row) + "\n")
    result = product_result(
        product,
        path,
        expected,
        {"answer.go", "unknown.go"},
        file_judgments={"Q1": [{"path": "answer.go", "grade": 3}]},
        judgment_policies={"Q1": "complete_ranked_pool_v1"},
    )
    assert result["per_query"][0]["eligible"] is False
    assert result["per_query"][0]["reason"] == "unjudged_ranked_file"
    assert result["file_hit_rate_at_10"] == "not_applicable"
    assert result["file_recall_at_10"] == "not_applicable"
    assert result["file_ndcg_at_10"] == "not_applicable"
    assert result["judgment_coverage"]["eligible_answerable"] == 0


def test_natural_language_native_defaults_cannot_issue_comparable_quality():
    from tools.benchmark.retrieval.lexical_file_comparison import comparison_validity

    products = {
        name: {"rank_unit": "distinct_file", "per_query": [{"task_id": "Q1", "eligible": True}]}
        for name in ("quanta_lexical", "semble_lexical_file", "sourcegraph", "opengrok", "cs")
    }
    result = comparison_validity({"Q1"}, {"Q1"}, products, "natural_language_file")
    assert result["status"] == "BLOCKED"
    assert "unequal_natural_language_query_semantics" in result["reasons"]
    assert result["quality_ranking_permitted"] is False


def test_partial_judgment_intersection_cannot_be_a_full_population_score():
    from tools.benchmark.retrieval.lexical_file_comparison import comparison_validity

    result = comparison_validity({"Q1", "Q2"}, {"Q1"}, {}, "code_search_file")
    assert result["status"] == "BLOCKED"
    assert "incomplete_common_judgments_or_capabilities" in result["reasons"]
    assert result["coverage_fraction"] == 0.5


def test_external_oracle_rescore_rejects_partial_or_off_view_judgments(tmp_path):
    expected = {"S01": ("Needle", ["src/original.go"])}
    path = tmp_path / "rows.jsonl"
    path.write_text("", encoding="utf-8")
    for scoring_gold in ({}, {"S01": ["src/outside.go"]}, {"S01": ["src/original.go"] * 2}):
        with pytest.raises(ValueError, match="invalid scoring gold inventory"):
            product_result(
                "sourcegraph",
                path,
                expected,
                {"src/original.go"},
                scoring_gold=scoring_gold,
            )


def test_external_oracle_manifest_binds_all_raw_files_and_rows(tmp_path):
    _base, _original, suite, pack = fixture_inputs(tmp_path)
    root = tmp_path / "capture"
    root.mkdir()
    suite_path, pack_path = tmp_path / "suite.json", tmp_path / "pack.json"
    suite_path.write_text(json.dumps(suite))
    pack_path.write_text(json.dumps(pack))
    paths = {
        "suite": suite_path,
        "query_pack": pack_path,
        "capture_manifest": root / "capture.json",
    }
    for product in ("sourcegraph", "opengrok", "cs"):
        row_path = root / f"{product}_rows.jsonl"
        row_path.write_text(product + "\n")
        paths[f"{product}_rows"] = row_path
    raw = {}
    for task in pack["tasks"]:
        for product, suffixes in (
            ("sourcegraph", ("stream", "transport.json")),
            ("opengrok", ("json", "transport.json")),
            ("cs", ("json", "process.json", "stderr")),
        ):
            for suffix in suffixes:
                name = f"{product}/{task['task_id']}.{suffix}"
                path = root / name
                path.parent.mkdir(exist_ok=True)
                path.write_text(name)
                raw[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    manifest = {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "tasks": len(pack["tasks"]),
        "binding": {
            "repository_commit": suite["repository_commit"],
            "file_universe_digest": "sha256:" + suite["file_universe_digest"],
            "suite_digest": "sha256:" + hashlib.sha256(suite_path.read_bytes()).hexdigest(),
            "query_pack_digest": "sha256:" + hashlib.sha256(pack_path.read_bytes()).hexdigest(),
        },
        "rows_sha256": {
            product: hashlib.sha256(paths[f"{product}_rows"].read_bytes()).hexdigest()
            for product in ("sourcegraph", "opengrok", "cs")
        },
        "raw_capture_sha256": raw,
    }
    paths["capture_manifest"].write_text(json.dumps(manifest))
    assert (
        verify_capture_manifest(paths, suite, pack)
        == hashlib.sha256(paths["capture_manifest"].read_bytes()).hexdigest()
    )
    (root / "sourcegraph" / "S00.stream").write_text("tampered")
    with pytest.raises(ValueError, match="raw response digest differs"):
        verify_capture_manifest(paths, suite, pack)
    (root / "sourcegraph" / "S00.stream").write_text("sourcegraph/S00.stream")
    paths["cs_rows"].write_text("tampered\n")
    with pytest.raises(ValueError, match="does not bind"):
        verify_capture_manifest(paths, suite, pack)


@pytest.mark.parametrize("result_path", ["../outside.go", "/outside.go", "src\\0.go", "other.go"])
def test_product_result_refuses_noncanonical_or_off_view_path(tmp_path, result_path):
    expected = {"S01": ("symbol", ["src/answer.go"])}
    path = tmp_path / "rows.jsonl"
    path.write_text(
        json.dumps(
            {
                "lane": "symbol_only",
                "task_id": "S01",
                "submitted_query": "symbol",
                "gold_paths": ["src/answer.go"],
                "http_status": 200,
                "error": None,
                "file_paths_top_10": [result_path],
                "file_hit_at_10": False,
                "elapsed_ms": 1.0,
            }
        )
        + "\n"
    )
    with pytest.raises(ValueError, match="malformed result paths"):
        product_result("sourcegraph", path, expected, {"src/answer.go"})


def test_symbol_diagnostic_refuses_non_bare_query(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    suite["tasks"][0]["query"] = "two words"
    suite["tasks"][0]["query_sha256"] = hashlib.sha256(b"two words").hexdigest()
    pack["tasks"][0]["query"] = "two words"
    pack["tasks"][0]["query_sha256"] = suite["tasks"][0]["query_sha256"]
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    with pytest.raises(ValueError, match="malformed blinded query"):
        _tasks(suite, pack)


def test_file_pair_task_admission_uses_bound_typo_policy(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    suite["routes"] = pack["routes"] = ["lexical", "semble-lexical-file"]
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    assert len(_tasks(suite, pack, file_policy="code_search_typo_file")) == 20
    suite["tasks"][0]["query"] = pack["tasks"][0]["query"] = "ab"
    invalid_digest = hashlib.sha256(b"ab").hexdigest()
    suite["tasks"][0]["query_sha256"] = pack["tasks"][0]["query_sha256"] = invalid_digest
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    with pytest.raises(ValueError, match="3..=64 bytes"):
        _tasks(suite, pack, file_policy="code_search_typo_file")


def nl_file_inputs(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    suite["routes"] = pack["routes"] = ["lexical", "semble-lexical-file"]
    for task, blinded in zip(suite["tasks"], pack["tasks"], strict=True):
        query = f"How does the implementation route incoming requests to the correct handler {task['task_id']}?"
        task["query"] = blinded["query"] = query
        task["query_sha256"] = blinded["query_sha256"] = hashlib.sha256(query.encode()).hexdigest()
        task["query_intent"] = "semantic_intent"
        task["evaluation_contract"] = {
            "request_mode": "natural_language_file_search",
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    return suite, pack


def test_native_file_tasks_admit_declared_nl_mode_without_rewriting_queries(tmp_path):
    suite, pack = nl_file_inputs(tmp_path)
    expected = _tasks(suite, pack)
    assert len(expected) == 20
    assert expected["S00"] == (
        "How does the implementation route incoming requests to the correct handler S00?",
        ["src/0.go"],
    )
    assert _tasks(suite, pack, file_policy="natural_language_file") == expected
    with pytest.raises(ValueError, match="native file policy differs"):
        _tasks(suite, pack, file_policy="code_search_file")


@pytest.mark.parametrize("fault", ["missing_mode", "mixed_mode", "chunk_unit"])
def test_native_nl_file_tasks_refuse_missing_mixed_or_wrong_unit_contract(tmp_path, fault):
    suite, pack = nl_file_inputs(tmp_path)
    task = suite["tasks"][0]
    if fault == "missing_mode":
        task.pop("evaluation_contract")
    elif fault == "mixed_mode":
        task["evaluation_contract"]["request_mode"] = "default_code_search"
    else:
        task["evaluation_contract"]["result_unit"] = "chunk"
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    with pytest.raises(ValueError, match="evaluation contract|native file policy"):
        _tasks(suite, pack, file_policy="natural_language_file")


def test_native_file_lock_accepts_nl_file_but_refuses_nl_chunk_policy():
    assert (
        _file_policy_from_lock(
            {"execution_profiles": {"quanta": {"policy": "natural_language_file"}}}
        )
        == "natural_language_file"
    )
    with pytest.raises(ValueError, match="supported file policy"):
        _file_policy_from_lock({"execution_profiles": {"quanta": {"policy": "natural_language"}}})


def test_file_diagnostic_accepts_small_repository_cell_but_refuses_empty(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    suite["routes"] = pack["routes"] = ["lexical", "semble-lexical-file"]
    suite["tasks"] = suite["tasks"][:8]
    pack["tasks"] = pack["tasks"][:8]
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    assert len(_tasks(suite, pack)) == 8

    suite["tasks"] = []
    pack["tasks"] = []
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    with pytest.raises(ValueError, match="empty"):
        _tasks(suite, pack)


def test_external_capture_refuses_explicit_typo_as_default_file_search(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    suite["routes"] = pack["routes"] = ["lexical", "semble-lexical-file"]
    suite["tasks"] = suite["tasks"][:8]
    pack["tasks"] = pack["tasks"][:8]
    for task in suite["tasks"]:
        task["query_intent"] = "bare_symbol"
        task["file_judgments"] = [{"path": task["gold"][0]["path"], "grade": 3}]
        task["evaluation_contract"] = {
            "request_mode": "explicit_osa1_typo",
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    with pytest.raises(ValueError, match="evaluation request mode"):
        _tasks(suite, pack)
    assert len(_tasks(suite, pack, file_policy="code_search_typo_file")) == 8


def test_symbol_diagnostic_distinguishes_judged_no_answer_from_unjudged(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    suite["tasks"][0]["gold"] = []
    suite["tasks"][0]["answerable"] = False
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    assert _tasks(suite, pack)["S00"] == ("symbol_0", [])
    suite["tasks"][0].pop("answerable")
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    with pytest.raises(ValueError, match="judged answerability"):
        _tasks(suite, pack)


def test_product_result_separates_answerable_recall_and_no_gold_empty_rate(tmp_path):
    expected = {"positive": ("symbol_a", ["answer.go"]), "negative": ("symbol_b", [])}
    rows = [
        {
            "lane": "symbol_only",
            "task_id": task_id,
            "submitted_query": query,
            "gold_paths": gold,
            "http_status": 200,
            "error": None,
            "file_paths_top_10": gold,
            "file_hit_at_10": bool(gold),
            "elapsed_ms": 1.0,
        }
        for task_id, (query, gold) in expected.items()
    ]
    path = tmp_path / "rows.jsonl"
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
    result = product_result("sourcegraph", path, expected, {"answer.go", "other.go"})
    assert result["answerable_tasks"] == result["no_gold_tasks"] == 1
    assert result["file_recall_at_10"] == result["file_hit_rate_at_10"] == 1.0
    assert result["no_gold_empty_rate_at_10"] == 1.0
    assert next(row for row in result["per_query"] if row["task_id"] == "negative") == {
        "task_id": "negative",
        "eligible": True,
        "file_hit_at_10": "not_applicable",
        "file_recall_at_10": "not_applicable",
        "no_gold_empty_at_10": True,
        "query_latency_ms": 1.0,
        "completed_query_latency_ms": None,
        "completed_response_boundary": None,
    }
    rows[1]["file_paths_top_10"] = ["other.go"]
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
    result = product_result("sourcegraph", path, expected, {"answer.go", "other.go"})
    assert result["file_recall_at_10"] == 1.0
    assert result["no_gold_empty_rate_at_10"] == 0.0
    path.write_text(json.dumps(rows[1]) + "\n")
    result = product_result("sourcegraph", path, {"negative": expected["negative"]}, {"other.go"})
    assert result["file_recall_at_10"] == "not_applicable"


def test_symbol_diagnostic_requires_native_top_10_contract(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    suite["comparison_contract"]["top_k"] = 20
    pack["comparison_contract"]["top_k"] = 20
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    with pytest.raises(ValueError, match="requires top_k 10"):
        _tasks(suite, pack)


def test_symbol_diagnostic_refuses_different_pack_contract_at_same_top_k(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    pack["comparison_contract"] = dict(pack["comparison_contract"])
    pack["comparison_contract"]["output_unit_policy"] = "unknown"
    with pytest.raises(ValueError, match="comparison contract"):
        _tasks(suite, pack)


@pytest.mark.parametrize(
    "field,value",
    [
        ("schema_version", 4),
        ("suite_id", "different-suite"),
        ("routes", ["hybrid"]),
        ("tokenizer", "other-tokenizer"),
        ("tokenizer_budget_version", "other-budget"),
    ],
)
def test_symbol_diagnostic_refuses_unbound_pack_metadata(tmp_path, field, value):
    _, _, suite, pack = fixture_inputs(tmp_path)
    pack[field] = value
    with pytest.raises(ValueError, match="pack metadata|route inventory"):
        _tasks(suite, pack)


def test_symbol_diagnostic_refuses_unbound_file_universe(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    files = [{"path": "src/answer.go", "file_sha256": "a" * 64}]
    suite["file_universe"] = files
    suite["file_universe_digest"] = digest(canonical(files))
    pack["file_universe"] = files
    pack["file_universe_digest"] = suite["file_universe_digest"]
    assert _file_universe(suite, pack) == {"src/answer.go"}
    pack["file_universe_digest"] = "b" * 64
    with pytest.raises(ValueError, match="file universe digest differs"):
        _file_universe(suite, pack)


@pytest.mark.parametrize("recall", [1.0, 0.5])
def test_pair_result_rejects_semantic_lane_even_if_report_has_hits(tmp_path, recall):
    _, _, suite, pack = fixture_inputs(tmp_path)
    if recall == 0.5:
        for task in suite["tasks"]:
            task["gold"].append({"path": "src/second-answer.go"})
        pack["suite_commitment_sha256"] = digest(canonical(suite))
    report = {
        "query_pack_sha256": digest(canonical(pack)),
        "runner_record_sha256": "f" * 64,
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "comparison_contract": suite["comparison_contract"],
        "sample_count": 20,
        "rank_metrics": {
            "routes": {
                route: {
                    "sample_count": 20,
                    "chunk": {"file_recall_at_10": recall},
                    "mean_query_latency_ms": 1.0,
                }
                for route in ("lexical", "semble-lexical-only")
            }
        },
        "per_query": [
            {
                "route": route,
                "task_id": task["task_id"],
                "query_latency_ms": 1.0,
                "file_recall_at_10": recall,
                "file_hit_at_10": True,
                "answerable": True,
                "status": "success",
                "candidates": 1,
            }
            for route in ("lexical", "semble-lexical-only")
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
        },
        "quanta_routes": ["lexical"],
        "semble_route": "semble-lexical-only",
        "top_k": 10,
        "strategies": ["fixed_window_strict"],
    }
    native = {
        "semble_profile": "lexical-only",
        "rerank_applied": False,
        "lane_call_counts": {"bm25": 20, "semantic": 0, "encode": 0},
        "execution_events": [
            {
                "phase": "measured",
                "task_id": task["task_id"],
                "lane_entry_counts": {"bm25": 1, "semantic": 0},
            }
            for task in pack["tasks"]
        ],
    }
    verdict = {
        "states": {"PAIR_VALID": "pass"},
        "counts": {"selected": 40, "executed": 40, "passed": 40, "failed": 0},
        "comparisons": [
            {
                "strategy": "fixed_window_strict",
                "candidate_route": "lexical",
                "baseline_route": "semble-lexical-only",
                "report_digest": hashlib.sha256(json.dumps(report).encode()).hexdigest(),
                "record_digest": report["runner_record_sha256"],
            }
        ],
    }
    paths = [tmp_path / f"{name}.json" for name in ("report", "lock", "native", "verdict")]
    for path, value in zip(paths, (report, lock, native, verdict), strict=True):
        path.write_text(json.dumps(value), encoding="utf-8")
    scored = pair_result(*paths, pack, suite, 20)
    assert scored["routes"]["quanta_lexical"]["hits"] == 20
    assert scored["routes"]["semble_lexical_only"]["hits"] == 20
    route = scored["routes"]["quanta_lexical"]
    assert route["rank_unit"] == "chunk"
    assert route["file_recall_at_10"] == recall
    assert route["file_hit_rate_at_10"] == 1.0
    missing_event = native["execution_events"].pop()
    native["lane_call_counts"]["bm25"] -= 1
    paths[2].write_text(json.dumps(native), encoding="utf-8")
    with pytest.raises(ValueError, match="measured task coverage"):
        pair_result(*paths, pack, suite, 20)
    native["execution_events"].append(missing_event)
    native["lane_call_counts"]["bm25"] += 1
    paths[2].write_text(json.dumps(native), encoding="utf-8")
    native["execution_events"][0]["task_id"] = []
    paths[2].write_text(json.dumps(native), encoding="utf-8")
    with pytest.raises(ValueError, match="measured task coverage"):
        pair_result(*paths, pack, suite, 20)
    native["execution_events"][0]["task_id"] = pack["tasks"][0]["task_id"]
    paths[2].write_text(json.dumps(native), encoding="utf-8")
    original_row = dict(report["per_query"][0])
    report["per_query"][0]["status"] = "capped"
    paths[0].write_text(json.dumps(report), encoding="utf-8")
    verdict["comparisons"][0]["report_digest"] = hashlib.sha256(paths[0].read_bytes()).hexdigest()
    paths[3].write_text(json.dumps(verdict), encoding="utf-8")
    assert pair_result(*paths, pack, suite, 20)["routes"]["quanta_lexical"]["hits"] == 20
    report["per_query"][0] = dict(original_row)
    for change in (
        {"answerable": False},
        {"status": "error", "candidates": 0},
        {"status": "success", "candidates": 0},
    ):
        report["per_query"][0].update(change)
        paths[0].write_text(json.dumps(report), encoding="utf-8")
        verdict["comparisons"][0]["report_digest"] = hashlib.sha256(
            paths[0].read_bytes()
        ).hexdigest()
        paths[3].write_text(json.dumps(verdict), encoding="utf-8")
        with pytest.raises(ValueError, match="recall/hit observations"):
            pair_result(*paths, pack, suite, 20)
        report["per_query"][0] = dict(original_row)
    paths[0].write_text(json.dumps(report), encoding="utf-8")
    verdict["comparisons"][0]["report_digest"] = hashlib.sha256(paths[0].read_bytes()).hexdigest()
    paths[3].write_text(json.dumps(verdict), encoding="utf-8")
    verdict["counts"]["passed"] = 39
    paths[3].write_text(json.dumps(verdict), encoding="utf-8")
    with pytest.raises(ValueError, match="execution counts differ"):
        pair_result(*paths, pack, suite, 20)
    verdict["counts"]["passed"] = 40
    verdict["comparisons"][0]["record_digest"] = "0" * 64
    paths[3].write_text(json.dumps(verdict), encoding="utf-8")
    with pytest.raises(ValueError, match="does not bind the lexical report"):
        pair_result(*paths, pack, suite, 20)
    verdict["comparisons"][0]["record_digest"] = report["runner_record_sha256"]
    paths[3].write_text(json.dumps(verdict), encoding="utf-8")
    lock["top_k"] = 20
    paths[1].write_text(json.dumps(lock), encoding="utf-8")
    with pytest.raises(ValueError, match="pair top_k contract differs"):
        pair_result(*paths, pack, suite, 20)
    lock["top_k"] = 10
    paths[1].write_text(json.dumps(lock), encoding="utf-8")
    report["per_query"][0]["file_recall_at_10"] = 0.0
    paths[0].write_text(json.dumps(report))
    with pytest.raises(ValueError, match="does not bind the lexical report"):
        pair_result(*paths, pack, suite, 20)
    verdict["comparisons"][0]["report_digest"] = hashlib.sha256(paths[0].read_bytes()).hexdigest()
    paths[3].write_text(json.dumps(verdict), encoding="utf-8")
    with pytest.raises(ValueError, match="recall/hit observations"):
        pair_result(*paths, pack, suite, 20)
    report["per_query"][0]["file_recall_at_10"] = recall
    paths[0].write_text(json.dumps(report))
    verdict["comparisons"][0]["report_digest"] = hashlib.sha256(paths[0].read_bytes()).hexdigest()
    paths[3].write_text(json.dumps(verdict), encoding="utf-8")
    native["lane_call_counts"]["semantic"] = 1
    paths[2].write_text(json.dumps(native), encoding="utf-8")
    with pytest.raises(ValueError, match="did not execute lexical-only"):
        pair_result(*paths, pack, suite, 20)


def test_pair_result_keeps_no_gold_out_of_recall_denominator(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    suite["tasks"][0]["gold"] = []
    suite["tasks"][0]["answerable"] = False
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    routes = ("lexical", "semble-lexical-only")
    report = {
        "query_pack_sha256": digest(canonical(pack)),
        "runner_record_sha256": "f" * 64,
        "comparison_contract": suite["comparison_contract"],
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "sample_count": 20,
        "rank_metrics": {
            "routes": {
                route: {
                    "sample_count": 20,
                    "chunk": {"file_recall_at_10": 1.0},
                    "mean_query_latency_ms": 1.0,
                }
                for route in routes
            }
        },
        "per_query": [
            {
                "route": route,
                "task_id": task["task_id"],
                "answerable": index != 0,
                "status": "abstained" if index == 0 else "success",
                "candidates": 0 if index == 0 else 1,
                "query_latency_ms": 1.0,
                "file_recall_at_10": "not_applicable" if index == 0 else 1.0,
                "file_hit_at_10": "not_applicable" if index == 0 else True,
            }
            for route in routes
            for index, task in enumerate(pack["tasks"])
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
        },
        "quanta_routes": ["lexical"],
        "semble_route": "semble-lexical-only",
        "top_k": 10,
        "strategies": ["fixed_window_strict"],
    }
    native = {
        "semble_profile": "lexical-only",
        "rerank_applied": False,
        "lane_call_counts": {"bm25": 20, "semantic": 0, "encode": 0},
        "execution_events": [
            {
                "phase": "measured",
                "task_id": task["task_id"],
                "lane_entry_counts": {"bm25": 1, "semantic": 0},
            }
            for task in pack["tasks"]
        ],
    }
    paths = [tmp_path / f"{name}.json" for name in ("report", "lock", "native", "verdict")]
    verdict = {
        "states": {"PAIR_VALID": "pass"},
        "counts": {"selected": 40, "executed": 40, "passed": 40, "failed": 0},
        "comparisons": [
            {
                "strategy": "fixed_window_strict",
                "candidate_route": "lexical",
                "baseline_route": "semble-lexical-only",
                "report_digest": "",
                "record_digest": report["runner_record_sha256"],
            }
        ],
    }

    def write_report():
        paths[0].write_text(json.dumps(report))
        verdict["comparisons"][0]["report_digest"] = digest(paths[0].read_bytes())
        paths[3].write_text(json.dumps(verdict))

    paths[1].write_text(json.dumps(lock))
    paths[2].write_text(json.dumps(native))
    write_report()
    result = pair_result(*paths, pack, suite, 20)["routes"]["quanta_lexical"]
    assert result["answerable_tasks"] == 19
    assert result["no_gold_tasks"] == 1
    assert result["file_recall_at_10"] == result["file_hit_rate_at_10"] == 1.0
    assert result["no_gold_empty_rate_at_10"] == 1.0
    report["per_query"][0].update(status="success", candidates=0)
    write_report()
    with pytest.raises(ValueError, match="recall/hit observations"):
        pair_result(*paths, pack, suite, 20)
    report["per_query"][0].update(status="abstained", candidates=0)
    positive = next(
        row for row in report["per_query"] if row["route"] == "lexical" and row["answerable"]
    )
    positive.update(status="abstained", candidates=0, file_recall_at_10=0.0, file_hit_at_10=False)
    report["rank_metrics"]["routes"]["lexical"]["chunk"]["file_recall_at_10"] = 18 / 19
    write_report()
    result = pair_result(*paths, pack, suite, 20)["routes"]["quanta_lexical"]
    assert result["hits"] == 18
    assert result["no_gold_empty_rate_at_10"] == 1.0
    positive.update(status="success", candidates=1, file_recall_at_10=1.0, file_hit_at_10=True)
    report["rank_metrics"]["routes"]["lexical"]["chunk"]["file_recall_at_10"] = 1.0
    report["per_query"][0]["file_recall_at_10"] = 0.0
    write_report()
    with pytest.raises(ValueError, match="recall/hit observations"):
        pair_result(*paths, pack, suite, 20)
    for task in suite["tasks"]:
        task["gold"] = []
        task["answerable"] = False
    pack["suite_commitment_sha256"] = digest(canonical(suite))
    report["query_pack_sha256"] = digest(canonical(pack))
    for route in routes:
        report["rank_metrics"]["routes"][route]["chunk"]["file_recall_at_10"] = "not_applicable"
    for row in report["per_query"]:
        row.update(
            answerable=False,
            status="abstained",
            candidates=0,
            file_recall_at_10="not_applicable",
            file_hit_at_10="not_applicable",
        )
    write_report()
    result = pair_result(*paths, pack, suite, 20)["routes"]["quanta_lexical"]
    assert result["answerable_tasks"] == 0
    assert result["file_recall_at_10"] == result["file_hit_rate_at_10"] == "not_applicable"
    assert result["no_gold_empty_rate_at_10"] == 1.0


def test_pair_result_rejects_hybrid_route_label_for_lexical_execution(tmp_path):
    _, _, suite, pack = fixture_inputs(tmp_path)
    report = {
        "query_pack_sha256": digest(canonical(pack)),
        "repository_commit": suite["repository_commit"],
        "file_universe_digest": suite["file_universe_digest"],
        "sample_count": 20,
        "rank_metrics": {"routes": {}},
        "per_query": [],
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
        },
        "quanta_routes": ["lexical"],
        "semble_route": "semble-hybrid",
    }
    native = {
        "semble_profile": "lexical-only",
        "rerank_applied": False,
        "lane_call_counts": {"bm25": 1, "semantic": 0, "encode": 0},
        "execution_events": [{"lane_entry_counts": {"bm25": 1, "semantic": 0}}],
    }
    verdict = {"states": {"PAIR_VALID": "pass"}}
    paths = [tmp_path / f"legacy-{name}.json" for name in ("report", "lock", "native", "verdict")]
    for path, value in zip(paths, (report, lock, native, verdict), strict=True):
        path.write_text(json.dumps(value), encoding="utf-8")
    with pytest.raises(ValueError, match="route labels do not match"):
        pair_result(*paths, pack, suite, 20)


def test_latency_summary_rejects_missing_and_nonfinite_values():
    values = [float(number) for number in range(1, 21)]
    summary = latency_summary(values, 20, "test-layer")
    assert summary["sum_ms"] == 210.0
    assert summary["p50_ms"] == 10.5
    assert summary["p95_ms"] == 19.0
    with pytest.raises(ValueError, match="invalid latency"):
        latency_summary(values[:-1], 20, "test-layer")
    with pytest.raises(ValueError, match="invalid latency"):
        latency_summary(values[:-1] + [float("nan")], 20, "test-layer")
    for invalid in (10**400, -(10**400), float("nan"), float("inf"), -float("inf"), True, None):
        with pytest.raises(ValueError, match="invalid latency"):
            latency_summary(values[:-1] + [invalid], 20, "test-layer")
    assert latency_summary([0, 1, 2.5], 3, "control")["count"] == 3
    with pytest.raises(ValueError, match="non-finite total latency"):
        latency_summary([1e308, 1e308], 2, "control")


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
    result = product_result("sourcegraph", path, expected, {"first.go", "second.go"})
    assert result["hits"] == 1
    assert result["file_hit_rate_at_10"] == 1.0
    assert result["file_recall_at_10"] == 0.5
    assert result["per_query"][0]["file_recall_at_10"] == 0.5


@pytest.mark.parametrize("raw", [b"[]\n", b'{"task_id":"q","task_id":"forged"}\n', b'{"x":NaN}\n'])
def test_malformed_or_duplicate_raw_rows_refuse(tmp_path, raw):
    path = tmp_path / "rows.jsonl"
    path.write_bytes(raw)
    with pytest.raises(ValueError):
        product_result("sourcegraph", path, {"q": ("symbol", ["answer.go"])}, {"answer.go"})


def test_lexical_product_consumes_lines_without_materializing_input(tmp_path, monkeypatch):
    path = tmp_path / "rows.jsonl"
    data = json.dumps(
        {
            "lane": "symbol_only",
            "task_id": "q",
            "submitted_query": "symbol",
            "gold_paths": ["answer.go"],
            "http_status": 200,
            "error": None,
            "file_paths_top_10": ["answer.go"],
            "file_hit_at_10": True,
            "elapsed_ms": 2.0,
        }
    ).encode()
    path.write_bytes(data)
    original = Path.read_bytes

    def bounded_only(value):
        assert value != path, "lexical observation whole read"
        return original(value)

    monkeypatch.setattr(Path, "read_bytes", bounded_only)
    result = product_result("sourcegraph", path, {"q": ("symbol", ["answer.go"])}, {"answer.go"})
    assert result["hits"] == result["tasks"] == 1
    assert result["latency_ms"]["mean_ms"] == 2.0
    assert result["raw_sha256"] == hashlib.sha256(data).hexdigest()


@pytest.mark.parametrize("tail", [b"\n", b'{"lane":', b"\xff"])
def test_lexical_stream_refuses_invalid_trailing_rows(tmp_path, tail):
    path = tmp_path / "rows.jsonl"
    row = {
        "lane": "symbol_only",
        "task_id": "q",
        "submitted_query": "symbol",
        "gold_paths": ["answer.go"],
        "http_status": 200,
        "error": None,
        "file_paths_top_10": ["answer.go"],
        "file_hit_at_10": True,
        "elapsed_ms": 2,
    }
    path.write_bytes(json.dumps(row).encode() + b"\n" + tail)
    with pytest.raises(ValueError):
        product_result("sourcegraph", path, {"q": ("symbol", ["answer.go"])}, {"answer.go"})


def test_lexical_control_and_line_limits_refuse_before_decode(tmp_path):
    from tools.benchmark.retrieval import lexical_file_comparison as owner

    path = tmp_path / "oversize"
    with path.open("wb") as stream:
        stream.truncate(owner.CONTROL_DOCUMENT_BYTES + 1)
    with pytest.raises(ValueError, match="control document exceeds"):
        owner._read(path)
    with pytest.raises(ValueError, match="line exceeds"):
        product_result("sourcegraph", path, {"q": ("symbol", ["answer.go"])}, {"answer.go"})


def test_lexical_result_metadata_has_separate_bound(tmp_path, monkeypatch):
    from tools.benchmark.retrieval import lexical_file_comparison as owner

    path = tmp_path / "rows.jsonl"
    path.write_text(
        json.dumps(
            {
                "lane": "symbol_only",
                "task_id": "q",
                "submitted_query": "symbol",
                "gold_paths": ["answer.go"],
                "http_status": 200,
                "error": None,
                "file_paths_top_10": ["answer.go"],
                "file_hit_at_10": True,
                "elapsed_ms": 2,
            }
        )
    )
    monkeypatch.setattr(owner, "CONTROL_DOCUMENT_BYTES", 1)
    with pytest.raises(ValueError, match="result metadata exceeds"):
        product_result("sourcegraph", path, {"q": ("symbol", ["answer.go"])}, {"answer.go"})


def test_lexical_stream_rss_does_not_retain_raw_responses(tmp_path, record_property):
    script = """
import json, resource, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from tools.benchmark.retrieval.lexical_file_comparison import product_result
count = int(sys.argv[3])
result = product_result('sourcegraph', Path(sys.argv[2]),
    {f'q-{index}': ('symbol', ['answer.go']) for index in range(count)}, {'answer.go'})
assert result['hits'] == result['tasks'] == len(result['per_query']) == count
assert result['file_recall_at_10'] == result['file_hit_rate_at_10'] == 1.0
assert result['latency_ms']['mean_ms'] == 2.0
peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
if sys.platform != 'darwin':
    peak *= 1024
print(json.dumps(dict(peak_bytes=peak, digest=result['raw_sha256'])))
"""
    peaks = []
    sizes = []
    for count in (120, 2040):
        path = tmp_path / f"rows-{count}.jsonl"
        expected_digest = hashlib.sha256()
        with path.open("wb") as stream:
            for index in range(count):
                data = (
                    json.dumps(
                        {
                            "lane": "symbol_only",
                            "task_id": f"q-{index}",
                            "submitted_query": "symbol",
                            "gold_paths": ["answer.go"],
                            "http_status": 200,
                            "error": None,
                            "file_paths_top_10": ["answer.go"],
                            "file_hit_at_10": True,
                            "elapsed_ms": 2,
                            "raw_response": "x" * 64000,
                        }
                    ).encode()
                    + b"\n"
                )
                stream.write(data)
                expected_digest.update(data)
        run = subprocess.run(
            [
                sys.executable,
                "-I",
                "-c",
                script,
                str(Path(__file__).resolve().parents[3]),
                str(path),
                str(count),
            ],
            capture_output=True,
            text=True,
            check=True,
            timeout=60,
        )
        measured = json.loads(run.stdout)
        assert measured["digest"] == expected_digest.hexdigest()
        assert measured["peak_bytes"] > 0
        peaks.append(measured["peak_bytes"])
        sizes.append(path.stat().st_size)
        record_property(f"lexical_{count}_bytes", sizes[-1])
        record_property(f"lexical_{count}_peak_bytes", peaks[-1])
    assert sizes[1] - sizes[0] > 100 * 1024 * 1024
    assert peaks[1] - peaks[0] < 32 * 1024 * 1024


def test_input_changed_during_read_cannot_bind_new_digest_to_old_score(tmp_path, monkeypatch):
    from tools.benchmark.retrieval import lexical_file_comparison as lexical
    from tools.ci.lint import handoff_validation

    path = tmp_path / "input.json"
    path.write_bytes(b'{"old":true}')
    original = handoff_validation._consume_repo_regular_file

    def mutate(root, value, *, label, consume):
        def changed(handle):
            data = consume(handle)
            if root / value == path:
                path.write_bytes(b'{"new":false}')
            return data

        return original(root, value, label=label, consume=changed)

    monkeypatch.setattr(handoff_validation, "_consume_repo_regular_file", mutate)
    with pytest.raises(ValueError, match="changed"):
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
