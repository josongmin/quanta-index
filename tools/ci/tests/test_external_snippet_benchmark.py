"""Fixed goldens and source/pack rejection for external snippet diagnostics."""

from __future__ import annotations

import hashlib
import json
import math
import subprocess

import pytest

from tools.benchmark.retrieval import (
    clarc_adapter as clarc,
)
from tools.benchmark.retrieval import (
    codesearchnet_materialize as csn_materialize,
)
from tools.benchmark.retrieval import (
    codesearchnet_qrels as csn_qrels,
)
from tools.benchmark.retrieval import (
    external_snippet_benchmark as ext,
)
from tools.benchmark.retrieval import (
    query_plan,
    retrieval_contract,
)


def _git(repo):
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "-A"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Benchmark Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
        check=True,
    )
    return subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()


def _clarc(tmp_path, monkeypatch):
    original = [
        {
            "query_id": f"q_group_1_id_{index}",
            "query_text": "Return a true value"
            if index == 0
            else " ".join(f"word{word}" for word in range(33)),
            "code_id": f"c_group_1_id_{index}",
            "code_text": f"int func{index}() {{ return {index}; }}",
            "relevance": 2,
        }
        for index in range(2)
    ]
    neutral = [
        {**row, "code_text": f"int neutral{index}() {{ return {index}; }}"}
        for index, row in enumerate(original)
    ]
    raws = {
        "original": json.dumps(original).encode(),
        "neutral_renamed": json.dumps(neutral).encode(),
        "dataset_card": b"---\nlicense: cc-by-sa-4.0\n---\n",
        "project_license_info": b"project_name,license_info,url\nexample,MIT,https://example.invalid\n",
    }
    monkeypatch.setattr(clarc, "EXPECTED_PAIRS", 2)
    monkeypatch.setattr(
        clarc,
        "SOURCES",
        {
            key: {"path": key, "sha256": hashlib.sha256(raw).hexdigest()}
            for key, raw in raws.items()
        },
    )
    data_root = tmp_path / "data"
    clarc.materialize(*raws.values(), data_root)
    repo = data_root / "original"
    commit = _git(repo)
    return data_root, repo, commit, raws


def test_clarc_native_pack_is_blind_source_bound_and_preserves_default_refusals(
    tmp_path, monkeypatch
):
    data_root, repo, commit, raws = _clarc(tmp_path, monkeypatch)
    pack, gold = ext.freeze_clarc(
        data_root, "original", repo, commit, raws, suite_id="clarc-fixture"
    )
    assert pack["schema_version"] == 3
    assert pack["repository_commit"] == commit
    assert (
        pack["file_universe_digest"]
        == hashlib.sha256(retrieval_contract.canonical(pack["file_universe"])).hexdigest()
    )
    assert len(pack["tasks"]) == 1
    assert gold["population_task_count"] == 2
    assert gold["admission"]["refused"] == 1
    assert gold["default_32_admission"]["refused_count"] == 1
    assert "gold" not in json.dumps(pack).lower()
    assert "c_group_1_id_0.cpp" not in json.dumps(pack["tasks"])
    frozen = ext.write_freeze(pack, gold, tmp_path / "frozen")
    assert json.loads((tmp_path / "frozen/runner/query-pack.json").read_text()) == pack
    assert json.loads((tmp_path / "frozen/scorer-input/gold-sidecar.json").read_text()) == gold
    assert list((tmp_path / "frozen/runner").iterdir()) == [
        tmp_path / "frozen/runner/query-pack.json"
    ]
    assert (tmp_path / "frozen/scorer-input/gold-sidecar.json").stat().st_mode & 0o777 == 0o600
    assert (
        frozen["query_pack_canonical_sha256"]
        == hashlib.sha256(retrieval_contract.canonical(pack)).hexdigest()
    )
    with pytest.raises(ext.ExternalSnippetError, match="new absolute root"):
        ext.write_freeze(pack, gold, tmp_path / "frozen")
    explicit = {"max_tokens": 64, "max_token_chars": 96, "min_token_chars": 1}
    pack64, gold64 = ext.freeze_clarc(
        data_root, "original", repo, commit, raws, suite_id="clarc-fixture-64", config=explicit
    )
    assert len(pack64["tasks"]) == 2
    assert gold64["admission"]["admitted"] == 2
    assert gold64["default_32_admission"]["refused_count"] == 1

    task_id = pack["tasks"][0]["task_id"]
    record = {
        "schema_version": 5,
        "query_pack_sha256": hashlib.sha256(retrieval_contract.canonical(pack)).hexdigest(),
        "comparison_contract": pack["comparison_contract"],
        "results": [
            {
                "task_id": task_id,
                "route": "lexical",
                "rank_unit": "distinct_file",
                "query_identity": {"original_query_sha256": pack["tasks"][0]["query_sha256"]},
                "status": "success",
                "candidates": [
                    {"rank": 1, "path": "snippets/c_group_1_id_1.cpp"},
                    {"rank": 2, "path": "snippets/c_group_1_id_0.cpp"},
                ],
            }
        ],
    }
    report = ext.score_capture(pack, gold, record)
    assert report["population_tasks"] == 2
    assert report["executed_scored"] == 1
    assert report["hit_at_10"] == 1
    assert report["mrr_at_10"] == 0.5
    assert report["unjudged_returned"] == 1
    assert report["operational_population_yield_lower_bound_hit_at_10"] == 0.5
    assert "pool_estimated_ndcg_at_10" not in report

    first, second = pack64["tasks"]
    mixed = {
        "schema_version": 5,
        "query_pack_sha256": hashlib.sha256(retrieval_contract.canonical(pack64)).hexdigest(),
        "comparison_contract": pack64["comparison_contract"],
        "results": [
            {
                "task_id": first["task_id"],
                "route": "lexical",
                "rank_unit": "distinct_file",
                "query_identity": {"original_query_sha256": first["query_sha256"]},
                "status": "success",
                "candidates": [{"rank": 1, "path": "snippets/c_group_1_id_0.cpp"}],
            },
            {
                "task_id": second["task_id"],
                "route": "lexical",
                "rank_unit": "distinct_file",
                "query_identity": {"original_query_sha256": second["query_sha256"]},
                "status": "error",
                "candidates": [],
            },
        ],
    }
    mixed_report = ext.score_capture(pack64, gold64, mixed)
    assert mixed_report["hit_at_10"] == 1.0
    assert mixed_report["execution_failed"] == 1
    assert mixed_report["operational_submitted_yield_lower_bound_hit_at_10"] == 0.5
    assert mixed_report["operational_population_yield_lower_bound_hit_at_10"] == 0.5

    corrupt = json.loads(json.dumps(record))
    corrupt["results"][0]["candidates"][1]["rank"] = 3
    with pytest.raises(ext.ExternalSnippetError, match="rank"):
        ext.score_capture(pack, gold, corrupt)
    corrupt = json.loads(json.dumps(record))
    corrupt["results"][0]["candidates"][0]["path"] = "missing.cpp"
    with pytest.raises(ext.ExternalSnippetError, match="unknown"):
        ext.score_capture(pack, gold, corrupt)
    corrupt = json.loads(json.dumps(record))
    corrupt["results"][0]["rank_unit"] = "chunk"
    with pytest.raises(ext.ExternalSnippetError, match="ranking unit"):
        ext.score_capture(pack, gold, corrupt)


def test_clarc_rejects_git_drift_and_blindpack_tamper(tmp_path, monkeypatch):
    data_root, repo, commit, raws = _clarc(tmp_path, monkeypatch)
    target = repo / "snippets/c_group_1_id_0.cpp"
    target.write_text("changed")
    with pytest.raises(ext.ExternalSnippetError, match="identity"):
        ext.freeze_clarc(data_root, "original", repo, commit, raws, suite_id="fixture")
    subprocess.run(["git", "-C", str(repo), "restore", "."], check=True)
    blind = data_root / "blindpack-original.json"
    payload = json.loads(blind.read_text())
    payload["tasks"][0]["query"] = "tampered"
    blind.write_text(json.dumps(payload))
    with pytest.raises(ext.ExternalSnippetError, match="blindpack query"):
        ext.freeze_clarc(data_root, "original", repo, commit, raws, suite_id="fixture")


def test_external_record_replay_binds_actual_profile_and_closed_shape(tmp_path, monkeypatch):
    data_root, repo, commit, raws = _clarc(tmp_path, monkeypatch)
    pack, gold = ext.freeze_clarc(
        data_root, "original", repo, commit, raws, suite_id="profile-fixture"
    )
    task = pack["tasks"][0]
    profile = query_plan.execution_profile(
        gold["admission"]["request_policy"], gold["admission"]["config"]
    )
    record = {
        "schema_version": 5,
        "query_pack_sha256": hashlib.sha256(retrieval_contract.canonical(pack)).hexdigest(),
        "comparison_contract": pack["comparison_contract"],
        "runner": {},
        "captures": {"capture": {"system": "quanta", "execution_profile": profile}},
        "route_provenance": {"lexical": {"capture_id": "capture"}},
        "results": [
            {
                "task_id": task["task_id"],
                "route": "lexical",
                "rank_unit": "distinct_file",
                "query_identity": {"original_query_sha256": task["query_sha256"]},
                "status": "success",
                "candidates": [{"rank": 1, "path": "snippets/c_group_1_id_0.cpp"}],
            }
        ],
    }
    # Profile binding is tested here; native record invariants have their own evaluator tests.
    monkeypatch.setattr(ext.evaluator, "_validate_run", lambda *_: None)
    assert ext.verify_and_score_capture(repo, pack, gold, record)["hit_at_10"] == 1

    wrong = json.loads(json.dumps(record))
    wrong["captures"]["capture"]["execution_profile"] = query_plan.execution_profile(
        "code_search_file"
    )
    with pytest.raises(ext.ExternalSnippetError, match="Quanta capture differs"):
        ext.verify_and_score_capture(repo, pack, gold, wrong)

    wrong = json.loads(json.dumps(record))
    wrong["captures"]["capture"]["execution_profile"] = query_plan.execution_profile(
        "natural_language_file", {"max_tokens": 64, "max_token_chars": 96, "min_token_chars": 1}
    )
    with pytest.raises(ext.ExternalSnippetError, match="Quanta capture differs"):
        ext.verify_and_score_capture(repo, pack, gold, wrong)

    wrong = json.loads(json.dumps(record))
    wrong["captures"]["capture"] = {
        "system": "semble",
        "execution_profile": {
            "profile_id": "semble-lexical-only-v1",
            "mode": "lexical-only",
            "alpha": None,
            "rerank": "not_applicable",
        },
    }
    with pytest.raises(ext.ExternalSnippetError, match="Semble capture is not lexical-file"):
        ext.verify_and_score_capture(repo, pack, gold, wrong)

    wrong = {**record, "unknown": True}
    with pytest.raises(ext.ExternalSnippetError, match="missing/unknown fields"):
        ext.verify_and_score_capture(repo, pack, gold, wrong)
    wrong = {key: value for key, value in record.items() if key != "runner"}
    with pytest.raises(ext.ExternalSnippetError, match="missing fields"):
        ext.verify_and_score_capture(repo, pack, gold, wrong)


def test_codesearchnet_fractional_qrels_language_split_and_partial_coverage(tmp_path, monkeypatch):
    sha40 = "0123456789abcdef0123456789abcdef01234567"
    urls = [
        f"https://github.com/example/project/blob/{sha40}/code.py#L{line}" for line in (1, 2, 3)
    ]
    missing_url = f"https://github.com/example/project/blob/{sha40}/missing.py#L1"
    qrels = [
        {"language": "python", "query": "find value", "github_url": urls[0], "mean_grade": 2.5},
        {"language": "python", "query": "find value", "github_url": urls[1], "mean_grade": 0.5},
        {"language": "python", "query": "read file", "github_url": urls[2], "mean_grade": 1.0},
        {"language": "python", "query": "read file", "github_url": missing_url, "mean_grade": 3.0},
        {"language": "go", "query": "find value", "github_url": urls[0], "mean_grade": 3.0},
    ]
    seed = {"source": {"sha256": "a" * 64}, "qrels": qrels}
    monkeypatch.setattr(csn_qrels, "diagnostic_seed", lambda _raw: seed)

    def fetch(url):
        if "missing.py" in url:
            return {"status": "http_error", "http_status": 404, "attempts": 1}
        return {
            "status": "fetched",
            "http_status": 200,
            "attempts": 1,
            "data": b"one\ntwo\nthree\n",
        }

    root = tmp_path / "materialized"
    manifest = csn_materialize.materialize(qrels, root, fetcher=fetch)
    manifest["upstream_csv"] = seed["source"]
    (root / "manifest.json").write_text(json.dumps(manifest))
    repo = tmp_path / "python-corpus"
    repo.mkdir()
    source_paths = {
        row["snippet_path"]
        for row in json.loads((root / "qrels.json").read_text())
        if row["language"] == "python" and row["snippet_path"] is not None
    }
    for path in source_paths:
        destination = repo / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes((root / path).read_bytes())
    commit = _git(repo)
    pack, gold = ext.freeze_codesearchnet(
        b"mock CSV", root, repo, commit, language="python", suite_id="csn-python-fixture"
    )
    assert gold["all_query_language_pairs"] == 3
    assert gold["qrels_total"] == 4
    assert gold["qrels_materialized"] == 3
    assert gold["population_task_count"] == 2
    assert gold["materialized_complete_tasks"] == 1
    assert {row["status"] for row in gold["full_population_ledger"]} == {
        "admitted",
        "blocked_source_unavailable",
    }
    assert sorted(row["grade"] for row in gold["judgments"][pack["tasks"][0]["task_id"]]) == [
        0.5,
        2.5,
    ]
    assert len(pack["tasks"]) == 1
    assert "grade" not in json.dumps(pack)
    task_id = pack["tasks"][0]["task_id"]
    judged = {row["grade"]: row["path"] for row in gold["judgments"][task_id]}
    unjudged = next(
        row["path"] for row in pack["file_universe"] if row["path"] not in set(judged.values())
    )
    record = {
        "schema_version": 5,
        "query_pack_sha256": hashlib.sha256(retrieval_contract.canonical(pack)).hexdigest(),
        "comparison_contract": pack["comparison_contract"],
        "results": [
            {
                "task_id": task_id,
                "route": "lexical",
                "rank_unit": "distinct_file",
                "query_identity": {"original_query_sha256": pack["tasks"][0]["query_sha256"]},
                "status": "success",
                "candidates": [
                    {"rank": 1, "path": unjudged},
                    {"rank": 2, "path": judged[0.5]},
                    {"rank": 3, "path": judged[2.5]},
                ],
            }
        ],
    }
    score = ext.score_capture(pack, gold, record)
    numerator = (2**0.5 - 1) / math.log2(3) + (2**2.5 - 1) / math.log2(4)
    denominator = (2**2.5 - 1) / math.log2(2) + (2**0.5 - 1) / math.log2(3)
    assert score["pool_estimated_ndcg_at_10"] == pytest.approx(numerator / denominator)
    assert score["hit_at_10"] == 1
    assert score["mrr_at_10"] == 0.5
    assert score["unjudged_returned"] == 1
    assert score["judged_returned_fraction"] == pytest.approx(2 / 3)
    assert score["population_tasks"] == 2 and score["materialized_complete_tasks"] == 1
    assert score["operational_submitted_yield_lower_bound_hit_at_10"] == 1.0
    assert score["operational_population_yield_lower_bound_hit_at_10"] == 0.5
    _, _, _, clarc_raws = _clarc(tmp_path, monkeypatch)
    prepared_root = tmp_path / "prepared"
    prepared = ext.prepare_external_lanes(
        clarc_raws, b"mock CSV", root, prepared_root, languages=("python",)
    )
    assert set(prepared["lanes"]) == {"clarc-original", "clarc-neutral_renamed", "csn-python"}
    assert prepared["execution_profile"]["config"]["max_tokens"] == 64
    assert prepared["codesearchnet_selected_population"] == 2
    assert prepared["codesearchnet_selected_complete"] == 1
    assert prepared["lanes"]["csn-python"]["source_blocked_tasks"] == 1
    assert prepared["runtime_gold_isolation"] == "not_verified_static_path_separation_only"
    assert json.loads((prepared_root / "manifest.json").read_text()) == prepared
    sources_path = root / "source-fetches.json"
    source_rows = json.loads(sources_path.read_text())
    source_url = next(url for url, value in source_rows.items() if value["status"] == "fetched")
    source_path = root / source_rows[source_url]["relative_path"]
    source_raw = source_path.read_bytes()
    outside = tmp_path / "outside.blob"
    outside.write_bytes(source_raw)
    for forged_path in (str(outside), "../outside.blob"):
        source_rows[source_url]["relative_path"] = forged_path
        sources_path.write_text(json.dumps(source_rows))
        with pytest.raises(ext.ExternalSnippetError, match="path differs from pinned URL"):
            ext.freeze_codesearchnet(
                b"mock CSV", root, repo, commit, language="python", suite_id="fixture"
            )
    source_rows[source_url]["relative_path"] = source_path.relative_to(root).as_posix()
    sources_path.write_text(json.dumps(source_rows))
    unavailable_url = next(
        url for url, value in source_rows.items() if value["status"] != "fetched"
    )
    source_rows[unavailable_url]["relative_path"] = str(outside)
    sources_path.write_text(json.dumps(source_rows))
    with pytest.raises(ext.ExternalSnippetError, match="unavailable CodeSearchNet source"):
        ext.freeze_codesearchnet(
            b"mock CSV", root, repo, commit, language="python", suite_id="fixture"
        )
    del source_rows[unavailable_url]["relative_path"]
    sources_path.write_text(json.dumps(source_rows))
    spans_path = root / "spans.json"
    spans = json.loads(spans_path.read_text())
    admitted_span = next(row for row in spans if row["status"] == "admitted")
    admitted_span["relative_paths"]["unlisted"] = str(outside)
    spans_path.write_text(json.dumps(spans))
    with pytest.raises(ext.ExternalSnippetError, match="snippet paths differ from pinned URL"):
        ext.freeze_codesearchnet(
            b"mock CSV", root, repo, commit, language="python", suite_id="fixture"
        )
    del admitted_span["relative_paths"]["unlisted"]
    spans_path.write_text(json.dumps(spans))
    source_path.unlink()
    source_path.symlink_to(outside)
    with pytest.raises(ext.ExternalSnippetError, match="symlink"):
        ext.freeze_codesearchnet(
            b"mock CSV", root, repo, commit, language="python", suite_id="fixture"
        )
    source_path.unlink()
    source_path.write_bytes(source_raw)
    snippet_path = root / next(iter(source_paths))
    snippet_raw = snippet_path.read_bytes()
    snippet_path.unlink()
    snippet_path.symlink_to(outside)
    with pytest.raises(ext.ExternalSnippetError, match="symlink"):
        ext.freeze_codesearchnet(
            b"mock CSV", root, repo, commit, language="python", suite_id="fixture"
        )
    snippet_path.unlink()
    snippet_path.write_bytes(snippet_raw)
    manifest_path = root / "manifest.json"
    manifest_raw = manifest_path.read_bytes()
    manifest_path.unlink()
    manifest_path.symlink_to(outside)
    with pytest.raises(ext.ExternalSnippetError, match="symlink"):
        ext.freeze_codesearchnet(
            b"mock CSV", root, repo, commit, language="python", suite_id="fixture"
        )
    manifest_path.unlink()
    manifest_path.write_bytes(manifest_raw)
    with pytest.raises(ext.ExternalSnippetError, match="supported language"):
        ext.freeze_codesearchnet(
            b"mock CSV", root, repo, commit, language="unknown", suite_id="fixture"
        )
    altered = json.loads((root / "qrels.json").read_text())
    altered[0]["mean_grade"] = 3.0
    (root / "qrels.json").write_text(json.dumps(altered))
    with pytest.raises(ext.ExternalSnippetError, match="pinned CSV"):
        ext.freeze_codesearchnet(
            b"mock CSV", root, repo, commit, language="python", suite_id="fixture"
        )


def test_official_codesearchnet_full_idcg_differs_from_pool_ndcg_at_10():
    judgments = [{"path": f"p{index}", "grade": 1.0} for index in range(11)]
    predictions = [f"p{index}" for index in range(10)]
    numerator = sum(1 / math.log2(rank + 1) for rank in range(1, 11))
    full_ideal = sum(1 / math.log2(rank + 1) for rank in range(1, 12))
    assert ext.official_csn_ndcg(predictions, judgments) == pytest.approx(numerator / full_ideal)
    assert ext.official_csn_ndcg(["unjudged", *predictions], judgments) == pytest.approx(
        numerator / full_ideal
    )
    assert ext.evaluator.file_ndcg_at_k(
        [{"path": path} for path in predictions], judgments, 10
    ) == pytest.approx(1.0)


def test_no_positive_judgment_is_undefined_not_no_answer(tmp_path, monkeypatch):
    _data, repo, commit, _raws = _clarc(tmp_path, monkeypatch)
    paths = [f"snippets/c_group_1_id_{index}.cpp" for index in range(2)]
    queries = ["Return a true value", "Find a second function"]
    tasks = [
        {
            "task_id": f"CSN-FIXTURE-{index}",
            "query": query,
            "query_sha256": hashlib.sha256(query.encode()).hexdigest(),
        }
        for index, query in enumerate(queries)
    ]
    pack, gold = ext._freeze(
        kind="codesearchnet_fractional_pool_v1",
        suite_id="csn-zero-positive-fixture",
        repo=repo,
        commit=commit,
        expected_files={path: (repo / path).read_bytes() for path in paths},
        tasks=tasks,
        judgments={
            tasks[0]["task_id"]: [{"path": paths[0], "grade": 0.0}],
            tasks[1]["task_id"]: [{"path": paths[1], "grade": 2.5}],
        },
        source={"fixture": True},
        config=None,
        top_k=10,
        extra={"qrels_total": 2, "qrels_materialized": 2, "all_query_language_pairs": 2},
    )
    record = {
        "schema_version": 5,
        "query_pack_sha256": hashlib.sha256(retrieval_contract.canonical(pack)).hexdigest(),
        "comparison_contract": pack["comparison_contract"],
        "results": [
            {
                "task_id": task["task_id"],
                "route": "lexical",
                "rank_unit": "distinct_file",
                "query_identity": {"original_query_sha256": task["query_sha256"]},
                "status": "success",
                "candidates": [{"rank": 1, "path": path}],
            }
            for task, path in zip(tasks, paths)
        ],
    }
    scored = ext.score_capture(pack, gold, record)
    assert scored["positive_known_complete_tasks"] == 1
    assert scored["no_positive_judgment_complete_tasks"] == 1
    assert scored["conditional_quality_defined_tasks"] == 1
    assert scored["per_query"][0]["judgment_state"] == (
        "no_positive_judgment_unjudged_pool_unknown"
    )
    assert scored["per_query"][0]["hit_at_10"] is None
    assert scored["per_query"][0]["mrr_at_10"] is None
    assert scored["per_query"][0]["pool_estimated_ndcg_at_10"] is None
    assert scored["hit_at_10"] == 1.0
    assert scored["operational_resolved_positive_hit_at_10"] == 1.0
    assert scored["operational_submitted_yield_lower_bound_hit_at_10"] == 0.5
