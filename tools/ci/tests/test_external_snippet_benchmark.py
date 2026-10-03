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
    assert json.loads((tmp_path / "frozen/owner-only/gold-sidecar.json").read_text()) == gold
    assert list((tmp_path / "frozen/runner").iterdir()) == [
        tmp_path / "frozen/runner/query-pack.json"
    ]
    assert (tmp_path / "frozen/owner-only/gold-sidecar.json").stat().st_mode & 0o777 == 0o600
    assert (
        frozen["query_pack_canonical_sha256"]
        == hashlib.sha256(retrieval_contract.canonical(pack)).hexdigest()
    )
    with pytest.raises(ext.ExternalSnippetError, match="new absolute root"):
        ext.write_freeze(pack, gold, tmp_path / "frozen")
    explicit = {"max_tokens": 128, "max_token_chars": 96, "min_token_chars": 1}
    pack128, gold128 = ext.freeze_clarc(
        data_root, "original", repo, commit, raws, suite_id="clarc-fixture-128", config=explicit
    )
    assert len(pack128["tasks"]) == 2
    assert gold128["admission"]["admitted"] == 2
    assert gold128["default_32_admission"]["refused_count"] == 1

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
    assert "pool_estimated_ndcg_at_10" not in report

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
