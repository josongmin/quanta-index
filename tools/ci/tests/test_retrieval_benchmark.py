"""Real Git source, blinded runner boundary and budgeted route scoring."""

from __future__ import annotations

import hashlib
import html
import json
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

import jsonschema
import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import run as pairrun
from tools.benchmark.retrieval import semble as semble_adapter
from tools.ci import source_closure


def test_duplicate_json_keys_refused(tmp_path):
    path = tmp_path / "duplicate.json"
    path.write_text('{"schema_version":3,"schema_version":3}', encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="duplicate JSON key"):
        ev.read_json(path)


def test_current_cli_requires_recorded_runner(tmp_path, capsys):
    repo, suite, _run, suite_path, runner_path, _files = fixture_v3(tmp_path)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    assert ev.main(["freeze", "--repo", str(repo), "--suite", str(suite_path)]) == 0
    pack = json.loads(capsys.readouterr().out)
    assert "gold" not in json.dumps(pack)
    assert "answerable" not in json.dumps(pack)
    assert (
        ev.main(
            [
                "evaluate",
                "--repo",
                str(repo),
                "--suite",
                str(suite_path),
                "--runner",
                str(runner_path),
                "--baseline-route",
                "lexical",
                "--candidate-route",
                "hybrid",
            ]
        )
        == 2
    )
    assert "cannot read JSON" in capsys.readouterr().err


def test_rank_prefix_does_not_skip_oversize_candidate():
    rows = [{"tokens": 3000}, {"tokens": 10}]
    assert ev.selected(rows, 2000) == ([], 0)


def test_token_unit_uses_explicit_ascii_ranges():
    assert ev.TOKEN_RE.findall("alpha_1 한글 !") == ["alpha_1", "한", "글", "!"]


# --- RB-01 current suite and scoring ---


def _write_repo(tmp_path: Path, files: dict[str, bytes]) -> tuple[Path, str]:
    repo = tmp_path / "source"
    repo.mkdir(parents=True, exist_ok=True)
    for name, data in files.items():
        (repo / name).write_bytes(data)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", *files.keys()], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "frozen",
        ],
        check=True,
    )
    return repo, ev.git(repo, "rev-parse", "HEAD")


def _span_meta(data: bytes, start: int, end: int) -> tuple[str, str, int]:
    lines = data.splitlines(keepends=True)
    selected = b"".join(lines[start - 1 : end])
    tokens = len(ev.TOKEN_RE.findall(selected.decode("utf-8")))
    return ev.digest(data), ev.digest(selected), tokens


def _byte_span(data: bytes, start: int, end: int) -> tuple[int, int]:
    lines = data.splitlines(keepends=True)
    first = sum(len(line) for line in lines[: start - 1])
    return first, first + sum(len(line) for line in lines[start - 1 : end])


def test_current_freeze_pack_is_blind(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    _, pack, _ = ev.validate_suite(repo, suite)
    text = json.dumps(pack)
    assert "gold" not in text
    assert "grade" not in text
    assert "answerable" not in text
    assert pack["schema_version"] == ev.SCHEMA_VERSION
    assert pack["tokenizer_budget_version"] == ev.TOKENIZER_BUDGET_VERSION
    assert pack["file_universe"] == suite["file_universe"]
    assert all(set(t) == {"task_id", "query", "query_sha256"} for t in pack["tasks"])


def test_current_hand_calculated_rank_metrics(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    loaded = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid")
    assert report["schema_version"] == ev.SCHEMA_VERSION
    assert report["rank_metric_version"] == "rb-rank-v2-first-coverage"
    assert report["graded"] is True
    assert report["primary_metric"] == "ndcg_at_10"
    routes = report["rank_metrics"]["routes"]
    # Lexical T1: [miss a:3 (same file, wrong lines), hit b:1]; hybrid T1: [hit a:2, miss, hit b:1].
    lex_chunk = routes["lexical"]["chunk"]
    hyb_chunk = routes["hybrid"]["chunk"]
    assert lex_chunk["recall_at_1"] == pytest.approx(0.0)
    assert lex_chunk["recall_at_5"] == pytest.approx(0.5)
    assert hyb_chunk["recall_at_1"] == pytest.approx(0.5)
    assert hyb_chunk["recall_at_5"] == pytest.approx(1.0)
    assert lex_chunk["mrr_at_10"] == pytest.approx(0.5)
    assert hyb_chunk["mrr_at_10"] == pytest.approx(1.0)
    import math as _math

    lex_dcg = 1 / _math.log2(3)
    hyb_dcg = 7.0 + 1 / 2
    idcg = 7.0 + 1 / _math.log2(3)
    assert lex_chunk["ndcg_at_10"] == pytest.approx(lex_dcg / idcg)
    assert hyb_chunk["ndcg_at_10"] == pytest.approx(hyb_dcg / idcg)
    # Same-file wrong lines earn file credit without span credit at rank 1 for lexical.
    assert lex_chunk["file_recall_at_10"] == pytest.approx(1.0)
    # Collapsed view keeps best chunk per file: hybrid collapsed NDCG is ideal.
    assert routes["hybrid"]["collapsed"]["ndcg_at_10"] == pytest.approx(1.0)
    assert routes["hybrid"]["collapsed"]["recall_at_5"] == pytest.approx(1.0)
    # BCY: hybrid covers every gold span, lexical covers only one.
    b4 = report["budgets"]["4000"]
    assert b4["routes"]["hybrid"]["bcy"] == 1
    assert b4["routes"]["lexical"]["bcy"] == 0
    assert b4["comparison"]["paired_wins"] == 2
    # Per-query rows and sample counts are explicit; small samples report NA intervals.
    assert report["sample_count"] == 2
    assert len(report["per_query"]) == 4
    assert report["rank_metrics"]["comparison"]["sample_count"] == 1
    assert (
        report["rank_metrics"]["comparison"]["primary_delta_ci_95"]["status"] == ev.NOT_APPLICABLE
    )


def test_ndcg_credits_each_gold_span_once_even_when_chunks_overlap():
    label = {"path": "src/lib.rs", "start_byte": 20, "end_byte": 30, "grade": 3}
    candidates = [
        {"path": "src/lib.rs", "start_byte": 0, "end_byte": 30},
        {"path": "src/lib.rs", "start_byte": 10, "end_byte": 40},
    ]
    assert ev.ndcg_at_k(candidates, [label], 10) == pytest.approx(1.0)
    assert ev.ndcg_at_k(list(reversed(candidates)), [label], 10) == pytest.approx(1.0)


def test_zero_grade_cannot_be_gold_in_suite_schema_or_evaluator(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)
    suite["tasks"][0]["gold"][0]["grade"] = 0
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(suite, _load_schema("suite.schema.json"))
    with pytest.raises(ev.EvidenceError, match="gold grade"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_bcy_budget_prefix_and_out_of_budget_not_credited(tmp_path):
    repo = tmp_path / "source"
    repo.mkdir()
    source = ("token " * 3000 + "\n").encode()
    (repo / "target.txt").write_bytes(source)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "target.txt"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "frozen",
        ],
        check=True,
    )
    commit = ev.git(repo, "rev-parse", "HEAD")
    label = {
        "path": "target.txt",
        "start_byte": 0,
        "end_byte": len(source),
        "start_line": 1,
        "end_line": 1,
        "file_sha256": ev.digest(source),
        "block_sha256": ev.digest(source),
        "grade": 2,
    }
    q1 = "find token handling"
    q2 = "find nonexistent"
    suite = {
        "schema_version": ev.SCHEMA_VERSION,
        "suite_id": "budget-current",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical", "hybrid"],
        "file_universe": [{"path": "target.txt", "file_sha256": ev.digest(source)}],
        "file_universe_digest": ev.universe_digest(
            [{"path": "target.txt", "file_sha256": ev.digest(source)}]
        ),
        "tasks": [
            {
                "task_id": "T1",
                "split": "eval",
                "query": q1,
                "query_sha256": ev.digest(q1.encode()),
                "query_family_id": "budget-answerable",
                "answerable": True,
                "gold": [label],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": q2,
                "query_sha256": ev.digest(q2.encode()),
                "query_family_id": "budget-no-answer",
                "answerable": False,
                "gold": [],
            },
        ],
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    cand = dict({k: v for k, v in label.items() if k != "grade"}, tokens=3000, rank=1)
    run = {
        "schema_version": ev.SCHEMA_VERSION,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "comparison_contract": _v3_contract(),
        "runner": {
            "name": "r",
            "revision": "r@1",
            "run_id": "run-1",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": "isolated",
            "isolation_method": "separate suite access",
            "access_block_log": "EACCES verified",
        },
        "captures": {"q0": _v3_capture("quanta")},
        "route_provenance": {
            "lexical": {"capture_id": "q0"},
            "hybrid": {"capture_id": "q0"},
        },
        "results": [
            {
                "task_id": "T1",
                "route": "lexical",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T1",
                "route": "hybrid",
                "status": "success",
                "candidates": [cand],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T2",
                "route": "lexical",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T2",
                "route": "hybrid",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
        ],
    }
    suite_path = tmp_path / "suite.json"
    runner_path = tmp_path / "run.json"
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    loaded = ev.load_evidence(repo, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid")
    assert report["budgets"]["2000"]["routes"]["hybrid"]["bcy"] == 0
    assert report["budgets"]["4000"]["routes"]["hybrid"]["bcy"] == 1
    # Rank metrics are budget-independent: the covering candidate scores at rank 1.
    assert report["rank_metrics"]["routes"]["hybrid"]["chunk"]["recall_at_1"] == pytest.approx(1.0)


def test_current_all_answerable_external_suite_passes_without_invented_no_answer(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path, answerable_only=True)
    loaded = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid")
    assert report["answerable_tasks"] == 2
    assert report["no_gold_tasks"] == 0
    assert report["budgets"]["4000"]["routes"]["hybrid"]["no_gold_abstention"] == ev.NOT_APPLICABLE
    assert (
        report["budgets"]["4000"]["comparison"]["delta"]["no_gold_abstention"] == ev.NOT_APPLICABLE
    )
    assert report["rank_metrics"]["routes"]["hybrid"]["chunk"]["recall_at_5"] == pytest.approx(1.0)


def test_current_same_candidates_identical_scores_regardless_of_runner(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    first = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    run["runner"]["name"] = "other-runner"
    run["runner"]["run_id"] = "run-other"
    run["captures"]["q0"]["model"] = "other-model"
    runner_path2 = tmp_path / "run2.json"
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path2.write_text(json.dumps(run), encoding="utf-8")
    second = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path2), "lexical", "hybrid")
    assert ev.canonical(first["budgets"]) == ev.canonical(second["budgets"])
    assert ev.canonical(first["rank_metrics"]) == ev.canonical(second["rank_metrics"])


def test_current_rescore_is_deterministic_under_row_order(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    first = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    run["results"] = list(reversed(run["results"]))
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    second = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path), "lexical", "hybrid")
    assert ev.canonical(first["budgets"]) == ev.canonical(second["budgets"])
    assert ev.canonical(first["rank_metrics"]) == ev.canonical(second["rank_metrics"])
    assert ev.canonical(first["per_query"]) == ev.canonical(second["per_query"])


def test_current_partial_span_earns_no_full_cover_credit(tmp_path):
    files = {"a.txt": b"line one\nline two\nline three\n"}
    repo, commit = _write_repo(tmp_path, files)
    file_sha = ev.digest(files["a.txt"])
    gold_sel = b"line one\nline two\n"
    part_sel = b"line one\n"
    q = "find first two lines"
    suite = {
        "schema_version": ev.SCHEMA_VERSION,
        "suite_id": "partial-current",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical", "hybrid"],
        "file_universe": [{"path": "a.txt", "file_sha256": file_sha}],
        "file_universe_digest": ev.universe_digest([{"path": "a.txt", "file_sha256": file_sha}]),
        "tasks": [
            {
                "task_id": "T1",
                "split": "eval",
                "query": q,
                "query_sha256": ev.digest(q.encode()),
                "query_family_id": "partial-answerable",
                "answerable": True,
                "gold": [
                    {
                        "path": "a.txt",
                        "start_byte": 0,
                        "end_byte": len(gold_sel),
                        "start_line": 1,
                        "end_line": 2,
                        "file_sha256": file_sha,
                        "block_sha256": ev.digest(gold_sel),
                        "grade": 3,
                    }
                ],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": "find nothing here",
                "query_sha256": ev.digest(b"find nothing here"),
                "query_family_id": "partial-no-answer",
                "answerable": False,
                "gold": [],
            },
        ],
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    part_tokens = len(ev.TOKEN_RE.findall(part_sel.decode()))
    full_tokens = len(ev.TOKEN_RE.findall(gold_sel.decode()))
    part = {
        "path": "a.txt",
        "start_byte": 0,
        "end_byte": len(part_sel),
        "start_line": 1,
        "end_line": 1,
        "file_sha256": file_sha,
        "block_sha256": ev.digest(part_sel),
        "tokens": part_tokens,
        "rank": 1,
    }
    full = {
        "path": "a.txt",
        "start_byte": 0,
        "end_byte": len(gold_sel),
        "start_line": 1,
        "end_line": 2,
        "file_sha256": file_sha,
        "block_sha256": ev.digest(gold_sel),
        "tokens": full_tokens,
        "rank": 1,
    }
    run = {
        "schema_version": ev.SCHEMA_VERSION,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "comparison_contract": _v3_contract(),
        "runner": {
            "name": "r",
            "revision": "r@1",
            "run_id": "x",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": "attested",
            "isolation_method": "attestation only",
            "access_block_log": "no separate suite access; attested-only",
        },
        "captures": {"q0": _v3_capture("quanta")},
        "route_provenance": {
            "lexical": {"capture_id": "q0"},
            "hybrid": {"capture_id": "q0"},
        },
        "results": [
            {
                "task_id": "T1",
                "route": "lexical",
                "status": "success",
                "candidates": [part],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T1",
                "route": "hybrid",
                "status": "success",
                "candidates": [full],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T2",
                "route": "lexical",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
            {
                "task_id": "T2",
                "route": "hybrid",
                "status": "abstained",
                "candidates": [],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            },
        ],
    }
    suite_path = tmp_path / "s.json"
    runner_path = tmp_path / "r.json"
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    report = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path), "lexical", "hybrid")
    lex = report["rank_metrics"]["routes"]["lexical"]["chunk"]
    hyb = report["rank_metrics"]["routes"]["hybrid"]["chunk"]
    assert lex["recall_at_1"] == pytest.approx(0.0)
    assert lex["file_recall_at_10"] == pytest.approx(1.0)
    assert hyb["recall_at_1"] == pytest.approx(1.0)
    assert report["budgets"]["4000"]["routes"]["lexical"]["bcy"] == 0
    assert report["budgets"]["4000"]["routes"]["hybrid"]["bcy"] == 1


def test_current_typed_failures_score_zero_and_are_counted(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    run["results"][1] = {
        "task_id": "T1",
        "route": "hybrid",
        "status": "timeout",
        "candidates": [],
        "timings": {"query_latency_ms": 5.0},
        "error": {"code": "TIMEOUT", "message": "deadline"},
    }
    report = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    hyb = report["rank_metrics"]["routes"]["hybrid"]
    assert hyb["chunk"]["recall_at_10"] == pytest.approx(0.0)
    assert hyb["status_counts"].get("timeout") == 1
    assert report["budgets"]["4000"]["routes"]["hybrid"]["bcy"] == 0
    rows = [r for r in report["per_query"] if r["task_id"] == "T1" and r["route"] == "hybrid"]
    assert rows[0]["error_code"] == "TIMEOUT"
    assert rows[0]["query_latency_ms"] == pytest.approx(5.0)


def test_current_capped_result_is_scored_but_flagged(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    run["results"][1]["status"] = "capped"
    report = ev.evaluate(*record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    assert report["rank_metrics"]["routes"]["hybrid"]["chunk"]["recall_at_5"] == pytest.approx(1.0)
    rows = [r for r in report["per_query"] if r["task_id"] == "T1" and r["route"] == "hybrid"]
    assert rows[0]["status"] == "capped"


def test_current_blinding_values_preserved(tmp_path):
    for blinding in ("isolated", "attested"):
        repo, suite, run, suite_path, runner_path, _ = fixture_v3(
            tmp_path / blinding, blinding=blinding
        )
        report = ev.evaluate(
            *record_v3(repo, suite, run, suite_path, runner_path), "lexical", "hybrid"
        )
        assert report["blinding"] == blinding
        assert report["runner"]["blinding"] == blinding


def test_current_confidence_interval_available_on_sufficient_sample(tmp_path):
    files = {"a.txt": b"alpha one\nalpha two\n"}
    repo, commit = _write_repo(tmp_path, files)
    file_sha = ev.digest(files["a.txt"])
    line1 = b"alpha one\n"
    tasks = []
    for i in range(20):
        q = "locate " + ev.digest(str(i).encode())
        tasks.append(
            {
                "task_id": f"T{i:02d}",
                "split": "eval",
                "query": q,
                "query_sha256": ev.digest(q.encode()),
                "query_family_id": f"ci-{i}",
                "answerable": True,
                "gold": [
                    {
                        "path": "a.txt",
                        "start_byte": 0,
                        "end_byte": len(line1),
                        "start_line": 1,
                        "end_line": 1,
                        "file_sha256": file_sha,
                        "block_sha256": ev.digest(line1),
                        "grade": 2,
                    }
                ],
            }
        )
    suite = {
        "schema_version": ev.SCHEMA_VERSION,
        "suite_id": "ci-current",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical", "hybrid"],
        "file_universe": [{"path": "a.txt", "file_sha256": file_sha}],
        "file_universe_digest": ev.universe_digest([{"path": "a.txt", "file_sha256": file_sha}]),
        "tasks": tasks,
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    tokens = len(ev.TOKEN_RE.findall(line1.decode()))
    miss_sel = b"alpha two\n"
    miss_tokens = len(ev.TOKEN_RE.findall(miss_sel.decode()))

    def hit(rank):
        return {
            "path": "a.txt",
            "start_byte": 0,
            "end_byte": len(line1),
            "start_line": 1,
            "end_line": 1,
            "file_sha256": file_sha,
            "block_sha256": ev.digest(line1),
            "tokens": tokens,
            "rank": rank,
        }

    def miss(rank):
        return {
            "path": "a.txt",
            "start_byte": len(line1),
            "end_byte": len(files["a.txt"]),
            "start_line": 2,
            "end_line": 2,
            "file_sha256": file_sha,
            "block_sha256": ev.digest(miss_sel),
            "tokens": miss_tokens,
            "rank": rank,
        }

    results = []
    for i in range(20):
        tid = f"T{i:02d}"
        # Lexical always hits; hybrid hits on even tasks only.
        results.append(
            {
                "task_id": tid,
                "route": "lexical",
                "status": "success",
                "candidates": [hit(1)],
                "timings": {"query_latency_ms": 1.0},
                "error": None,
            }
        )
        if i % 2 == 0:
            results.append(
                {
                    "task_id": tid,
                    "route": "hybrid",
                    "status": "success",
                    "candidates": [hit(1)],
                    "timings": {"query_latency_ms": 1.0},
                    "error": None,
                }
            )
        else:
            results.append(
                {
                    "task_id": tid,
                    "route": "hybrid",
                    "status": "success",
                    "candidates": [miss(1)],
                    "timings": {"query_latency_ms": 1.0},
                    "error": None,
                }
            )
    run = {
        "schema_version": ev.SCHEMA_VERSION,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "comparison_contract": _v3_contract(),
        "runner": {
            "name": "r",
            "revision": "r@1",
            "run_id": "ci",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": "isolated",
            "isolation_method": "separate suite access",
            "access_block_log": "verified",
        },
        "captures": {"q0": _v3_capture("quanta")},
        "route_provenance": {
            "lexical": {"capture_id": "q0"},
            "hybrid": {"capture_id": "q0"},
        },
        "results": results,
    }
    suite_path = tmp_path / "s.json"
    runner_path = tmp_path / "r.json"
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    report = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path), "lexical", "hybrid")
    ci = report["rank_metrics"]["comparison"]["primary_delta_ci_95"]
    assert ci["sample_count"] == 20
    assert ci["method"] == "paired_stratified_bootstrap_percentile_v1"
    assert ci["resamples"] == 10_000
    assert ci["strata"] == {"uncategorized": 20}
    assert len(ci["seed_sha256"]) == 64
    assert ci["lower_95"] <= ci["mean"] <= ci["upper_95"]
    repeated = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path), "lexical", "hybrid")[
        "rank_metrics"
    ]["comparison"]["primary_delta_ci_95"]
    assert repeated == ci
    assert report["rank_metrics"]["comparison"]["paired_losses"] == 10
    stratified = report["rank_metrics"]["comparison"]["stratified_primary_delta"]
    assert stratified["category"]["uncategorized"]["sample_count"] == 20
    assert stratified["language"]["txt"]["sample_count"] == 20
    assert stratified["repository"][commit]["sample_count"] == 20
    no_answer = report["rank_metrics"]["comparison"]["no_answer_abstention_delta"]
    assert no_answer["sample_count"] == 0
    assert no_answer["mean_delta"] == ev.NOT_APPLICABLE
    assert no_answer["strata"] == {"category": {}, "language": {}, "repository": {}}


def test_bootstrap_is_order_independent_and_rejects_invalid_pairs(monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 4)
    deltas = [0.4, -0.2, 0.1, 0.7]
    strata = [("T2", "semantic"), ("T1", "symbol"), ("T4", "semantic"), ("T3", "symbol")]
    expected = ev.mean_ci(deltas, strata)
    assert ev.mean_ci(list(reversed(deltas)), list(reversed(strata))) == expected
    with pytest.raises(ev.EvidenceError, match="identities must be unique"):
        ev.mean_ci(deltas, [("T1", "a"), ("T1", "b"), ("T3", "a"), ("T4", "b")])
    with pytest.raises(ev.EvidenceError, match="deltas must be finite"):
        ev.mean_ci([0.0, 1.0, float("nan"), 2.0], strata)


def test_stratified_delta_summary_binds_category_language_and_repository(monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    rows = [
        ("T1", {"category": "symbol", "gold": [{"path": "src/a.rs"}]}, 0.5),
        ("T2", {"category": "symbol", "gold": [{"path": "src/b.rs"}]}, -0.5),
        ("T3", {"category": "semantic", "gold": [{"path": "pkg/c.py"}]}, 0.25),
        (
            "T4",
            {
                "category": "semantic",
                "gold": [{"path": "pkg/d.py"}, {"path": "web/e.ts"}],
            },
            0.75,
        ),
    ]
    commit = "a" * 40
    summary = ev.stratified_delta_summary(rows, commit)
    assert set(summary) == {"category", "language", "repository"}
    assert summary["category"]["symbol"]["sample_count"] == 2
    assert summary["category"]["symbol"]["mean_delta"] == pytest.approx(0.0)
    assert summary["category"]["semantic"]["mean_delta"] == pytest.approx(0.5)
    assert summary["language"]["rs"]["sample_count"] == 2
    assert summary["language"]["py"]["sample_count"] == 1
    assert summary["language"]["mixed:py+ts"]["sample_count"] == 1
    assert summary["repository"][commit]["sample_count"] == 4
    assert summary["repository"][commit]["ci_95"].get("status") != ev.NOT_APPLICABLE
    assert ev.stratified_delta_summary(list(reversed(rows)), commit) == summary

    no_answer_rows = [
        ("N1", {"category": "negative", "gold": []}, 1.0),
        ("N2", {"category": "negative", "gold": []}, 0.0),
    ]
    no_answer = ev.no_answer_delta_summary(no_answer_rows, commit)
    assert no_answer["metric"] == "no_answer_abstention"
    assert no_answer["sample_count"] == 2
    assert no_answer["mean_delta"] == pytest.approx(0.5)
    assert no_answer["strata"]["language"]["not_applicable:no_gold"]["sample_count"] == 2


def test_qualified_uncertainty_contract_rejects_incomplete_or_forged_strata(monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    rows = [
        ("T1", {"category": "symbol", "gold": [{"path": "a.rs"}]}, 0.25),
        ("T2", {"category": "symbol", "gold": [{"path": "b.rs"}]}, -0.25),
    ]
    comparison = {
        "primary_delta": 0.0,
        "sample_count": 2,
        "paired_wins": 1,
        "paired_losses": 1,
        "paired_ties": 0,
        "primary_delta_ci_95": ev.mean_ci([0.25, -0.25], [("T1", "symbol"), ("T2", "symbol")]),
        "stratified_primary_delta": ev.stratified_delta_summary(rows, "a" * 40),
        "no_answer_abstention_delta": ev.no_answer_delta_summary([], "a" * 40),
    }
    assert pairrun._qualified_uncertainty(comparison)

    mutants = []
    for mutate in (
        lambda value: value.update(sample_count=1),
        lambda value: value.update(primary_delta=0.5),
        lambda value: value.update(paired_ties=1),
        lambda value: value["primary_delta_ci_95"].update(method="normal_approximation"),
        lambda value: value["primary_delta_ci_95"].update(resamples=100),
        lambda value: value["primary_delta_ci_95"].update(seed_sha256="bad"),
        lambda value: value["primary_delta_ci_95"].update(strata={"symbol": 1}),
        lambda value: value["primary_delta_ci_95"].update(lower_95=2.0),
        lambda value: value["primary_delta_ci_95"].update(mean=0.5),
        lambda value: value["stratified_primary_delta"].pop("language"),
        lambda value: value["stratified_primary_delta"]["category"].clear(),
        lambda value: value["stratified_primary_delta"]["category"]["symbol"].pop("mean_delta"),
        lambda value: value["stratified_primary_delta"]["category"]["symbol"].update(
            mean_delta=0.5
        ),
        lambda value: value["stratified_primary_delta"]["category"]["symbol"]["ci_95"].update(
            mean=0.5
        ),
        lambda value: value["stratified_primary_delta"]["category"]["symbol"].update(
            sample_count=1
        ),
        lambda value: value.pop("no_answer_abstention_delta"),
        lambda value: value["no_answer_abstention_delta"].update(sample_count=1),
        lambda value: value["no_answer_abstention_delta"]["ci_95"].update(method="normal"),
        lambda value: value["no_answer_abstention_delta"]["strata"]["category"].update(
            forged={"sample_count": 1, "mean_delta": 0.0, "ci_95": {}}
        ),
    ):
        mutant = json.loads(json.dumps(comparison))
        mutate(mutant)
        mutants.append(mutant)
    assert all(not pairrun._qualified_uncertainty(mutant) for mutant in mutants)

    no_answer_comparison = json.loads(json.dumps(comparison))
    no_answer_comparison["no_answer_abstention_delta"] = ev.no_answer_delta_summary(
        [
            ("N1", {"category": "negative", "gold": []}, 1.0),
            ("N2", {"category": "negative", "gold": []}, 0.0),
        ],
        "a" * 40,
    )
    assert pairrun._qualified_uncertainty(no_answer_comparison)
    for mutate in (
        lambda value: value.update(mean_delta=0.75),
        lambda value: value["strata"]["category"]["negative"].update(mean_delta=0.75),
        lambda value: value["strata"]["repository"]["a" * 40].update(sample_count=1),
    ):
        mutant = json.loads(json.dumps(no_answer_comparison))
        mutate(mutant["no_answer_abstention_delta"])
        assert not pairrun._qualified_uncertainty(mutant)


@pytest.mark.parametrize(
    "mutation,match",
    [
        (lambda s, r, f: s.update(repository_commit="0" * 40), "checkout HEAD differs"),
        (
            lambda s, r, f: s["tasks"][0]["gold"][0].update(block_sha256="0" * 64),
            "block hash mismatch",
        ),
        (
            lambda s, r, f: s["tasks"][0]["gold"][0].update(file_sha256="0" * 64),
            "file hash mismatch",
        ),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(grade=5), "grade"),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(grade=0), "gold grade"),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(grade="high"), "grade"),
        (lambda s, r, f: s["file_universe"][0].update(file_sha256="0" * 64), "file universe"),
        (lambda s, r, f: s["tasks"][1].update(answerable=True), "answerable/gold mismatch"),
        (
            lambda s, r, f: s["tasks"][1].update(
                query=s["tasks"][0]["query"], query_sha256=s["tasks"][0]["query_sha256"]
            ),
            "query leakage",
        ),
        (lambda s, r, f: r["results"].pop(), "missing task route evidence"),
        (
            lambda s, r, f: r["results"].append(dict(r["results"][0])),
            "unexpected/duplicate task route",
        ),
        (lambda s, r, f: r["results"][1]["candidates"][1].update(rank=1), "rank"),
        (lambda s, r, f: r["results"][1]["candidates"][0].update(tokens=1), "token count mismatch"),
        (
            lambda s, r, f: r["results"][1]["candidates"][0].update(file_sha256="0" * 64),
            "file hash mismatch",
        ),
        (lambda s, r, f: r["results"][1]["candidates"][0].update(gold=True), "unknown fields"),
        (lambda s, r, f: r["results"][1].update(gold=[]), "unknown fields"),
        (lambda s, r, f: r["runner"].update(tokenizer="other-tok"), "tokenizer"),
        (lambda s, r, f: r["runner"].update(tokenizer_budget_version="qb-x"), "tokenizer"),
        (lambda s, r, f: r["runner"].update(blinding="none"), "blinding"),
        (lambda s, r, f: r["route_provenance"].pop("hybrid"), "route_provenance"),
        (lambda s, r, f: r["results"][1].update(status="mystery"), "unknown result status"),
        (lambda s, r, f: r["results"][1]["timings"].update(query_latency_ms=-1), "timing"),
        (lambda s, r, f: r["results"][1]["timings"].update(query_latency_ms="fast"), "timing"),
        (lambda s, r, f: r.update(query_pack_sha256="0" * 64), "query pack hash mismatch"),
        (lambda s, r, f: s.update(suite_id="changed"), "query pack hash mismatch"),
        (lambda s, r, f: r.__setitem__("schema_version", 1), "unsupported runner schema"),
    ],
)
def test_current_fails_closed_on_invalid_evidence(tmp_path, mutation, match):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    mutation(suite, run, files)
    with pytest.raises(ev.EvidenceError, match=match):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_excluded_but_tracked_candidate_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    file_sha, block_sha, tokens = _span_meta(files["excluded.txt"], 1, 1)
    run["results"][1]["candidates"][0] = {
        "path": "excluded.txt",
        "start_byte": 0,
        "end_byte": len(files["excluded.txt"]),
        "start_line": 1,
        "end_line": 1,
        "file_sha256": file_sha,
        "block_sha256": block_sha,
        "tokens": tokens,
        "rank": 1,
    }
    # Re-sequence remaining ranks to isolate the universe failure.
    for i, cand in enumerate(run["results"][1]["candidates"], start=1):
        cand["rank"] = i
    with pytest.raises(ev.EvidenceError, match="excluded from file universe"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_gold_in_excluded_file_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    file_sha, block_sha, _ = _span_meta(files["excluded.txt"], 1, 1)
    suite["tasks"][0]["gold"][0] = {
        "path": "excluded.txt",
        "start_byte": 0,
        "end_byte": len(files["excluded.txt"]),
        "start_line": 1,
        "end_line": 1,
        "file_sha256": file_sha,
        "block_sha256": block_sha,
        "grade": 2,
    }
    with pytest.raises(ev.EvidenceError, match="excluded from file universe"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_unsafe_candidate_path_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    run["results"][1]["candidates"][0]["path"] = "../a.txt"
    with pytest.raises(ev.EvidenceError, match="unsafe repository path"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_error_result_with_candidates_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path)
    run["results"][1]["status"] = "error"
    run["results"][1]["error"] = {"code": "E", "message": "m"}
    with pytest.raises(ev.EvidenceError, match="cannot contain candidates"):
        record_v3(repo, suite, run, suite_path, runner_path)


def test_current_cli_freeze_evaluate_roundtrip(tmp_path, capsys):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    pack_path = tmp_path / "pack.json"
    report_path = tmp_path / "report.json"
    assert (
        ev.main(
            ["freeze", "--repo", str(repo), "--suite", str(suite_path), "--output", str(pack_path)]
        )
        == 0
    )
    assert capsys.readouterr().out.strip() == run["query_pack_sha256"]
    text = pack_path.read_text(encoding="utf-8")
    assert "gold" not in text
    assert "grade" not in text
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    assert (
        ev.main(
            [
                "evaluate",
                "--repo",
                str(repo),
                "--suite",
                str(suite_path),
                "--runner",
                str(runner_path),
                "--baseline-route",
                "lexical",
                "--candidate-route",
                "hybrid",
                "--output",
                str(report_path),
            ]
        )
        == 0
    )
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert report["schema_version"] == ev.SCHEMA_VERSION
    assert report["rank_metrics"]["comparison"]["sample_count"] == 1


# --- RB-04/RB-05 adapter and merge contracts (T11–T13) ---


def _admitted_rows(files: dict[str, bytes]) -> list[tuple[str, str]]:
    return [(name, ev.digest(data)) for name, data in sorted(files.items())]


def test_mapping_proof_clean_and_mismatch_detected(tmp_path):
    _repo, _suite, _run, _sp, _rp, files = fixture_v3(tmp_path)
    corpus = tmp_path / "corpus"
    corpus.mkdir()
    for name, data in files.items():
        if name != "excluded.txt":
            (corpus / name).write_bytes(data)
    admitted = _admitted_rows({k: v for k, v in files.items() if k != "excluded.txt"})
    proof, diff = semble_adapter.mapping_proof(admitted, ["a.txt", "b.txt"], corpus)
    assert proof["skipped"] == [] and proof["extra"] == []
    assert proof["mismatched"] == []
    assert len(diff) == 64
    assert all(row["status"] == "indexed" for row in proof["per_file"])

    skipped, _ = semble_adapter.mapping_proof(admitted, ["a.txt"], corpus)
    assert skipped["skipped"] == ["b.txt"]
    extra, _ = semble_adapter.mapping_proof(admitted, ["a.txt", "b.txt", "zzz.txt"], corpus)
    assert extra["extra"] == ["zzz.txt"]
    assert extra["semble_side"][-1]["readable"] is False

    (corpus / "a.txt").write_bytes(b"different bytes")
    changed, _ = semble_adapter.mapping_proof(admitted, ["a.txt", "b.txt"], corpus)
    assert changed["skipped"] == changed["extra"] == []
    assert changed["mismatched"] == ["a.txt"]
    assert changed["per_file"][0]["status"] == "hash_mismatch"
    manifest = {"files": [{"path": name, "file_sha256": sha} for name, sha in admitted]}
    assert pairrun.mapping_matches_manifest(proof, manifest)
    assert not pairrun.mapping_matches_manifest(changed, manifest)
    tampered = dict(
        proof,
        semble_side=[dict(proof["semble_side"][0], file_sha256="0" * 64), proof["semble_side"][1]],
    )
    assert not pairrun.mapping_matches_manifest(tampered, manifest)


def test_semble_inputs_refuse_duplicate_keys_and_unsafe_paths(tmp_path):
    path = tmp_path / "manifest.json"
    path.write_text('{"tasks":[{"gold":[]}],"tasks":[]}', encoding="utf-8")
    with pytest.raises(semble_adapter.AdapterError, match="duplicate JSON key: tasks"):
        semble_adapter.read_json(path)
    path.write_text(
        json.dumps(
            {
                "repository_commit": "a" * 40,
                "files": [{"path": "../outside.rs", "file_sha256": "b" * 64}],
            }
        ),
        encoding="utf-8",
    )
    with pytest.raises(semble_adapter.AdapterError, match="unsafe manifest path"):
        semble_adapter.load_manifest(path)


def test_pair_driver_refuses_ambiguous_json(tmp_path):
    path = tmp_path / "run-manifest.json"
    path.write_text(
        '{"evidence":{"pair":{"same_files":false}},"evidence":{"pair":{"same_files":true}}}',
        encoding="utf-8",
    )
    with pytest.raises(pairrun.RunError, match="duplicate JSON key: evidence"):
        pairrun.read_json(path)
    path.write_text('{"query_latency_ms":NaN}', encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="non-finite JSON number: NaN"):
        pairrun.read_json(path)


def test_pair_capture_preflight_requires_external_root_and_clean_pin(tmp_path):
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path)
    manifest = tmp_path / "manifest.json"
    manifest.write_text(
        json.dumps({"repository_commit": suite["repository_commit"]}), encoding="utf-8"
    )
    suite_file = tmp_path / "suite.json"
    suite_file.write_text(json.dumps(suite), encoding="utf-8")
    searchd = tmp_path / "searchd"
    searchd.write_bytes(b"searchd-binary")
    spec = {
        "repo": str(repo),
        "manifest": str(manifest),
        "suite": str(suite_file),
        "top_k": 10,
        "output_root": str(repo / "capture"),
        "searchd_binary": str(searchd),
        "searchd_expected_sha256": ev.digest(b"searchd-binary"),
    }
    with pytest.raises(pairrun.RunError, match="outside the frozen repository"):
        pairrun.preflight_capture(spec)
    spec["output_root"] = str(tmp_path / "capture")
    assert pairrun.preflight_capture(spec) == tmp_path / "capture"
    (repo / "untracked.txt").write_text("dirty", encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="checkout has tracked or untracked changes"):
        pairrun.preflight_capture(spec)
    (repo / "untracked.txt").unlink()
    spec["top_k"] = 5
    with pytest.raises(pairrun.RunError, match="spec top_k differs"):
        pairrun.preflight_capture(spec)
    spec["top_k"] = 10
    spec["searchd_expected_sha256"] = "0" * 64
    with pytest.raises(pairrun.RunError, match="searchd binary digest differs"):
        pairrun.preflight_capture(spec)


def test_semble_model_revision_requires_observed_pinned_cache(tmp_path):
    model = "minishlab/potion-code-16M-v2"
    with pytest.raises(semble_adapter.AdapterError, match="revision unavailable"):
        semble_adapter.resolve_model_revision(tmp_path, model, None)
    ref = tmp_path / "hub/models--minishlab--potion-code-16M-v2/refs/main"
    ref.parent.mkdir(parents=True)
    ref.write_text("a" * 40, encoding="utf-8")
    with pytest.raises(semble_adapter.AdapterError, match="snapshot unavailable"):
        semble_adapter.resolve_model_revision(tmp_path, model, "a" * 40)
    snap = tmp_path / ("hub/models--minishlab--potion-code-16M-v2/snapshots/" + "a" * 40)
    snap.mkdir(parents=True)
    (snap / "config.json").write_bytes(b"{}")
    revision, asset = semble_adapter.resolve_model_revision(tmp_path, model, "a" * 40)
    assert revision == "a" * 40
    assert len(asset) == 64
    with pytest.raises(semble_adapter.AdapterError, match="revision drift"):
        semble_adapter.resolve_model_revision(tmp_path, model, "b" * 40)


def test_semble_env_refuses_missing_lockfile(tmp_path, monkeypatch):
    interpreter = tmp_path / "python"
    interpreter.write_text("", encoding="utf-8")

    def fake_run(command, **kwargs):
        if "-c" in command:
            return subprocess.CompletedProcess(
                command,
                0,
                '{"semble_version":"0.6.0","python_version":"3.11","has_from_path":true,"has_search":true,'
                '"dist_info":"semble-0.6.0.dist-info","record_sha256":"%s","direct_url_sha256":null}'
                % ("a" * 64),
                "",
            )
        return subprocess.CompletedProcess(command, 1, "", "pip failed")

    monkeypatch.setattr(semble_adapter.subprocess, "run", fake_run)
    with pytest.raises(semble_adapter.AdapterError, match="pip freeze failed"):
        semble_adapter.check_semble_env(interpreter)


def test_semble_env_reports_lockfile_digest_and_pair_requires_pin(tmp_path, monkeypatch):
    interpreter = tmp_path / "python"
    interpreter.write_text("", encoding="utf-8")

    installed = (
        '"dist_info":"semble-0.6.0.dist-info","record_sha256":"%s","direct_url_sha256":null'
    ) % ("b" * 64)

    def fake_run(command, **kwargs):
        if "-c" in command:
            return subprocess.CompletedProcess(
                command,
                0,
                '{"semble_version":"0.6.0","python_version":"3.11","has_from_path":true,"has_search":true,'
                + installed
                + "}",
                "",
            )
        return subprocess.CompletedProcess(command, 0, "semble==0.6.0\n", "")

    monkeypatch.setattr(semble_adapter.subprocess, "run", fake_run)
    report = semble_adapter.check_semble_env(interpreter)
    assert report["observed_freeze_sha256"] == ev.digest(b"semble==0.6.0\n")
    assert report["installed_distribution"]["record_sha256"] == "b" * 64
    assert report["interpreter"]["digest"] == ev.digest(b"")
    assert report["interpreter"]["version"] == "3.11"

    def wrong_version(command, **_kwargs):
        if "-c" in command:
            return subprocess.CompletedProcess(
                command,
                0,
                '{"semble_version":"0.7.0","python_version":"3.11","has_from_path":true,"has_search":true}',
                "",
            )
        return subprocess.CompletedProcess(command, 0, "semble==0.7.0\n", "")

    monkeypatch.setattr(semble_adapter.subprocess, "run", wrong_version)
    with pytest.raises(semble_adapter.AdapterError, match="0.6.0 is pinned"):
        semble_adapter.check_semble_env(interpreter)
    with pytest.raises(pairrun.RunError, match="requires a pinned semble_lockfile_sha256"):
        pairrun.run_pair({"output_root": str(tmp_path / "out")})
    assert not (tmp_path / "out").exists()


def test_verify_lockfile_pins_external_file_not_freeze():
    pin_file = b"semble==0.6.0\nnumpy==2.0.0\n# generated lock\n"
    pin = ev.digest(pin_file)
    freeze = "numpy==2.0.0\nsemble==0.6.0\nextra==1.0\n"
    assert semble_adapter.verify_lockfile(pin_file, pin, freeze) == pin
    with pytest.raises(semble_adapter.AdapterError, match="differs from the spec pin"):
        semble_adapter.verify_lockfile(b"semble==0.6.0\n", pin, freeze)
    with pytest.raises(semble_adapter.AdapterError, match="lacks the pinned Semble line"):
        semble_adapter.verify_lockfile(pin_file, pin, "numpy==2.0.0\n")
    with pytest.raises(semble_adapter.AdapterError, match="lacks 1 locked lines"):
        semble_adapter.verify_lockfile(pin_file, pin, "semble==0.6.0\nextra==1.0\n")


def test_check_semble_env_refuses_missing_installed_proof(tmp_path, monkeypatch):
    interpreter = tmp_path / "python"
    interpreter.write_text("", encoding="utf-8")

    def run_with(probe_json):
        def fake_run(command, **_kwargs):
            if "-c" in command:
                return subprocess.CompletedProcess(command, 0, probe_json, "")
            return subprocess.CompletedProcess(command, 0, "semble==0.6.0\n", "")

        monkeypatch.setattr(semble_adapter.subprocess, "run", fake_run)
        return semble_adapter.check_semble_env(interpreter)

    base = (
        '{"semble_version":"0.6.0","python_version":"3.11","has_from_path":true,"has_search":true'
    )
    with pytest.raises(semble_adapter.AdapterError, match="dist-info identity proof"):
        run_with(base + "}")
    with pytest.raises(semble_adapter.AdapterError, match="RECORD digest proof"):
        run_with(base + ',"dist_info":"semble-0.6.0.dist-info","record_sha256":null}')
    with pytest.raises(semble_adapter.AdapterError, match="malformed direct_url digest"):
        run_with(
            base + ',"dist_info":"semble-0.6.0.dist-info",'
            f'"record_sha256":"{"c" * 64}","direct_url_sha256":"zz"}}'
        )
    report = run_with(
        base + ',"dist_info":"semble-0.6.0.dist-info",'
        f'"record_sha256":"{"d" * 64}","direct_url_sha256":"{"e" * 64}"}}'
    )
    assert report["installed_distribution"]["direct_url_sha256"] == "e" * 64


def test_load_query_pack_refuses_duplicate_task_ids(tmp_path):
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path, answerable_only=True)
    _s, pack, _src = ev.validate_suite(repo, suite)
    assert len(pack["tasks"]) >= 1
    pack["tasks"].append(dict(pack["tasks"][0]))
    pack_path = tmp_path / "pack.json"
    pack_path.write_text(json.dumps(pack), encoding="utf-8")
    with pytest.raises(semble_adapter.AdapterError, match="duplicate task_id"):
        semble_adapter.load_query_pack(pack_path)


def test_load_query_pack_refuses_smuggled_labels(tmp_path):
    # T01: the blind adapter refuses packs carrying gold/grade or any
    # unexpected key, at top level and per task.
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path, answerable_only=True)
    _s, pack, _src = ev.validate_suite(repo, suite)
    pack_path = tmp_path / "pack.json"

    def attempt(mutator, match):
        mutated = json.loads(json.dumps(pack))
        mutator(mutated)
        pack_path.write_text(json.dumps(mutated), encoding="utf-8")
        with pytest.raises(semble_adapter.AdapterError, match=match):
            semble_adapter.load_query_pack(pack_path)

    attempt(lambda p: p["tasks"][0].update(gold=[]), "smuggling refused")
    attempt(lambda p: p["tasks"][0].update(grade=3), "smuggling refused")
    attempt(lambda p: p.update(gold=[]), "unexpected or missing top-level keys")
    pack_path.write_text(json.dumps(pack), encoding="utf-8")
    assert semble_adapter.load_query_pack(pack_path)["tasks"]


def test_worker_template_runs_against_stub_semble(tmp_path, monkeypatch):
    (tmp_path / "semble.py").write_text(
        "class _Chunk:\n"
        "    def __init__(self, file_path, start_line, end_line):\n"
        "        self.file_path = file_path\n"
        "        self.start_line = start_line\n"
        "        self.end_line = end_line\n"
        "class _Hit:\n"
        "    def __init__(self, chunk, score):\n"
        "        self.chunk = chunk\n"
        "        self.score = score\n"
        "class _Stats:\n"
        "    indexed_files = 1\n"
        "    total_chunks = 1\n"
        "    languages = {'txt': 1}\n"
        "class SembleIndex:\n"
        "    @classmethod\n"
        "    def from_path(cls, corpus_dir, show_progress_bar=False):\n"
        "        self = cls()\n"
        "        self._resident_index = bytearray(64 * 1024 * 1024)\n"
        "        self.chunks = [_Chunk('a.txt', 1, 1)]\n"
        "        self.stats = _Stats()\n"
        "        return self\n"
        "    def search(self, query, top_k=10):\n"
        "        return [_Hit(_Chunk('a.txt', 1, 1), 0.5)]\n",
        encoding="utf-8",
    )
    worker = tmp_path / "worker.py"
    worker.write_text(semble_adapter.WORKER_TEMPLATE, encoding="utf-8")
    spec = {
        "corpus_dir": str(tmp_path),
        "tasks": [{"task_id": "T1", "query": "q"}],
        "top_k": 5,
        "seed": 0,
        "warmup_passes": 1,
        "repetitions": 2,
        "query_protocol": pairrun.build_query_protocol(["T1"], 0, 1, 2),
    }
    spec_path = tmp_path / "spec.json"
    native_path = tmp_path / "native.json"
    spec_path.write_text(json.dumps(spec), encoding="utf-8")
    monkeypatch.setenv("SPEC_JSON", str(spec_path))
    monkeypatch.setenv("NATIVE_JSON", str(native_path))
    monkeypatch.setenv("SEMBLE_MODEL_NAME", "stub-model")
    monkeypatch.setenv("PYTHONPATH", str(tmp_path))

    def run_worker():
        return subprocess.run(
            [sys.executable, str(worker)],
            capture_output=True,
            text=True,
            timeout=60,
        )

    completed = run_worker()
    assert completed.returncode == 0, completed.stderr
    payload = json.loads(native_path.read_text(encoding="utf-8"))
    assert payload["timing_layer"] == "worker_wall_per_query_ms"
    assert payload["worker_pid"] > 0
    assert [row["task_id"] for row in payload["native"]] == ["T1"]
    assert len(payload["latencies_ms"]["T1"]) == 2
    assert payload["query_protocol"] == spec["query_protocol"]
    assert payload["cold_latency_ms"] >= 0
    assert payload["native"][0]["results"][0]["file_path"] == "a.txt"
    assert payload["observed_files"] == ["a.txt"]

    spec["tasks"].append({"task_id": "T1", "query": "q2"})
    spec_path.write_text(json.dumps(spec), encoding="utf-8")
    duplicate = run_worker()
    assert duplicate.returncode != 0
    assert "duplicate task_ids" in duplicate.stderr


def test_run_pair_requires_lockfile_path(tmp_path):
    base = {
        "output_root": str(tmp_path / "out"),
        "semble_python": "/venv/bin/python",
        "semble_lockfile_sha256": "a" * 64,
    }
    with pytest.raises(pairrun.RunError, match="spec.semble_lockfile"):
        pairrun.run_pair(dict(base))
    with pytest.raises(pairrun.RunError, match="spec.host_profile"):
        pairrun.run_pair(dict(base, semble_lockfile=str(tmp_path / "lock.txt")))


def test_run_semble_capture_forwards_lockfile(tmp_path, monkeypatch):
    seen = {}

    def fake_run(command, **kwargs):
        seen["command"] = command
        output_root = Path(command[command.index("--output-root") + 1])
        output_root.mkdir(exist_ok=True)
        (output_root / "phase-metrics.json").write_text("{}", encoding="utf-8")
        (output_root / "native.json").write_text(
            json.dumps(
                {
                    "stats": {
                        "indexed_files": 1,
                        "total_chunks": 1,
                        "index_resident_bytes": 4096,
                        "index_measurement": "process_peak_rss_delta_v1",
                    }
                }
            ),
            encoding="utf-8",
        )
        kwargs["resource_path"].write_text("{}", encoding="utf-8")
        return {"exit_code": 0, "timed_out": False}

    monkeypatch.setattr(pairrun, "run_monitored_process", fake_run)
    spec = {
        "repo": "r",
        "manifest": "m",
        "top_k": 10,
        "semble_python": "/venv/bin/python",
        "semble_lockfile": "/frozen/semble-lockfile.txt",
        "semble_lockfile_sha256": "c" * 64,
    }
    pairrun.run_semble_capture(spec, tmp_path, tmp_path / "pack.json", "hybrid")
    command = seen["command"]
    assert command[command.index("--lockfile") + 1] == "/frozen/semble-lockfile.txt"
    assert command[command.index("--lockfile-sha256") + 1] == "c" * 64
    assert command[command.index("--repetitions") + 1] == "1"
    assert command[command.index("--warmup-passes") + 1] == "1"


def test_freeze_inputs_freezes_lockfile(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    (src / "lock.txt").write_bytes(b"semble==0.6.0\n")
    for name in ("suite.json", "pack.json", "manifest.json"):
        (src / name).write_text("{}", encoding="utf-8")
    (src / "host-profile.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "profile_id": "test",
                "fingerprint": {
                    "system": "Darwin",
                    "release": "test",
                    "machine": "arm64",
                    "processor": "test",
                    "cpu_count": 8,
                    "rustc": "rustc test",
                    "power_digest": _fake_sha("power"),
                },
            }
        ),
        encoding="utf-8",
    )
    inputs = {
        "suite": str(src / "suite.json"),
        "query_pack": str(src / "pack.json"),
        "manifest": str(src / "manifest.json"),
        "semble_lockfile": str(src / "lock.txt"),
        "host_profile": str(src / "host-profile.json"),
    }
    stage = tmp_path / "stage"
    stage.mkdir()
    frozen = pairrun.freeze_inputs(inputs, stage)
    assert frozen["suite"] == str(stage / "evaluator-only" / "suite.json")
    assert frozen["semble_lockfile"] == str(stage / "semble-lockfile.txt")
    assert (stage / "semble-lockfile.txt").read_bytes() == b"semble==0.6.0\n"
    stage2 = tmp_path / "stage2"
    stage2.mkdir()
    with pytest.raises(pairrun.RunError, match="cannot freeze capture input semble_lockfile"):
        pairrun.freeze_inputs(dict(inputs, semble_lockfile=str(tmp_path / "absent.txt")), stage2)


def test_quanta_driver_defaults_to_potion_and_binary_digest(tmp_path, monkeypatch):
    def fake_run(command, **kwargs):
        assert command[command.index("--embedder") + 1] == "potion-code"
        assert command[command.index("--runner-revision") + 1] == "sha256:" + "a" * 64
        assert command[command.index("--searchd-bin") + 1] == "/unused/searchd"
        assert command[command.index("--searchd-expected-sha256") + 1] == "b" * 64
        Path(command[command.index("--out") + 1]).write_text("{}", encoding="utf-8")
        Path(command[command.index("--metrics-out") + 1]).write_text("{}", encoding="utf-8")
        kwargs["resource_path"].write_text("{}", encoding="utf-8")
        return {"exit_code": 0, "timed_out": False, "elapsed_ms": 1.0}

    monkeypatch.setattr(pairrun, "run_monitored_process", fake_run)
    spec = {
        "runner_binary": "/unused/runner",
        "repo": "/unused/repo",
        "manifest": "/unused/manifest.json",
        "top_k": 10,
        "searchd_binary": "/unused/searchd",
        "searchd_expected_sha256": "b" * 64,
    }
    result = pairrun.run_quanta_strategy(
        spec, {"name": "whole_file"}, 0, tmp_path, ["lexical"], tmp_path / "pack.json", "a" * 64
    )
    assert result["runner_binary_sha256"] == "a" * 64
    for legacy in ("syntax", "fixed_window"):
        with pytest.raises(pairrun.RunError, match="unknown strategy"):
            pairrun.run_quanta_strategy(
                spec, {"name": legacy}, 0, tmp_path, ["lexical"], tmp_path / "pack.json", "a" * 64
            )


def test_quanta_driver_freezes_typed_failure_without_record(tmp_path, monkeypatch):
    def fake_run(_command, **kwargs):
        kwargs["stdout_path"].write_text("", encoding="utf-8")
        kwargs["stderr_path"].write_text("provider unavailable", encoding="utf-8")
        kwargs["resource_path"].write_text('{"exit_code":2}', encoding="utf-8")
        return {"exit_code": 2, "timed_out": False, "elapsed_ms": 1.0}

    monkeypatch.setattr(pairrun, "run_monitored_process", fake_run)
    spec = {
        "runner_binary": "/unused/runner",
        "repo": "/unused/repo",
        "manifest": "/unused/manifest.json",
        "top_k": 10,
        "searchd_binary": "/unused/searchd",
        "searchd_expected_sha256": "b" * 64,
    }
    with pytest.raises(pairrun.RunError, match="Rust runner failed"):
        pairrun.run_quanta_strategy(
            spec,
            {"name": "whole_file"},
            0,
            tmp_path,
            ["lexical"],
            tmp_path / "pack.json",
            "a" * 64,
        )
    failure = json.loads(
        (tmp_path / "strategy-00-whole_file" / "failure.json").read_text(encoding="utf-8")
    )
    assert failure["failure_type"] == "nonzero_exit"
    assert failure["record_emitted"] is False


def test_process_tree_resource_sampler_counts_children_and_kills_timeout(tmp_path):
    child_code = (
        "import subprocess,sys,time; "
        "subprocess.Popen([sys.executable,'-c','x=bytearray(8_000_000); time.sleep(5)']); "
        "time.sleep(5)"
    )
    metrics = pairrun.run_monitored_process(
        [sys.executable, "-c", child_code],
        stdout_path=tmp_path / "stdout.log",
        stderr_path=tmp_path / "stderr.log",
        resource_path=tmp_path / "resource.json",
        timeout_secs=1,
        sample_interval_ms=20,
    )
    assert metrics["timed_out"] is True
    assert metrics["exit_code"] != 0
    assert metrics["complete"] is True
    assert metrics["samples"] > 0
    assert metrics["peak_rss_bytes"] > 8_000_000
    assert metrics["cleanup_complete"] is True
    assert metrics["cleanup_error"] is None
    assert (
        subprocess.run(["ps", "-p", str(metrics["root_pid"])], capture_output=True).returncode != 0
    )


@pytest.mark.skipif(
    pairrun.platform.system() != "Darwin" or not pairrun.SANDBOX_EXEC.is_file(),
    reason="macOS Seatbelt backend is unavailable",
)
def test_isolation_boundary_denies_suite_and_allows_blind_pack(tmp_path):
    stage = tmp_path / "capture.staging"
    evaluator = stage / "evaluator-only"
    evaluator.mkdir(parents=True)
    frozen_suite = evaluator / "suite.json"
    frozen_suite.write_text('{"gold":"secret"}', encoding="utf-8")
    pack = stage / "query-pack.json"
    pack.write_text('{"query":"blind"}', encoding="utf-8")
    secret_root = tmp_path / "secret"
    secret_root.mkdir()
    original_suite = secret_root / "suite.json"
    original_suite.write_bytes(frozen_suite.read_bytes())
    source_repo = tmp_path / "repo"
    source_repo.mkdir()
    duplicate_gold = source_repo / "duplicate-gold.json"
    duplicate_gold.write_text('{"gold":"secret"}', encoding="utf-8")
    repo = stage / "runner-corpus"
    repo.mkdir()
    admitted = repo / "a.txt"
    admitted.write_text("admitted", encoding="utf-8")
    manifest = stage / "corpus-manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "repository_commit": "a" * 40,
                "files": [{"path": "a.txt", "file_sha256": pairrun.sha_file(admitted)}],
            }
        ),
        encoding="utf-8",
    )
    materialized = pairrun._verify_materialized_corpus(repo, manifest)
    inputs = {}
    for name in ("runner_binary", "searchd_binary", "semble_python", "semble_lockfile"):
        path = tmp_path / name
        path.write_text(name, encoding="utf-8")
        inputs[name] = str(path)
    spec = {
        **inputs,
        "repo": str(repo),
        "manifest": str(manifest),
        "_source_repo": str(source_repo),
        "_materialized_corpus": materialized,
        "suite": str(frozen_suite),
        "query_pack": str(pack),
        "output_root": str(tmp_path / "final"),
        "blinding": "isolated",
        "suite_secret_root": str(secret_root),
    }
    prepared = pairrun.prepare_isolation(spec, stage, original_suite)
    assert prepared["isolation_method"] == pairrun.ISOLATION_BACKEND
    proof_path = stage / "isolation-proof.json"
    assert prepared["access_block_log"] == f"sha256:{pairrun.sha_file(proof_path)}"
    denied_command, evidence = pairrun.sandbox_command(prepared, ["/bin/cat", str(original_suite)])
    denied = subprocess.run(denied_command, capture_output=True, text=True, timeout=15)
    assert denied.returncode != 0
    assert denied.stdout == ""
    denied_source, _ = pairrun.sandbox_command(prepared, ["/bin/cat", str(duplicate_gold)])
    source_attempt = subprocess.run(denied_source, capture_output=True, text=True, timeout=15)
    assert source_attempt.returncode != 0
    assert source_attempt.stdout == ""
    allowed_corpus, _ = pairrun.sandbox_command(prepared, ["/bin/cat", str(admitted)])
    admitted_attempt = subprocess.run(allowed_corpus, capture_output=True, text=True, timeout=15)
    assert admitted_attempt.returncode == 0
    assert admitted_attempt.stdout == "admitted"
    assert evidence["proof_sha256"] == pairrun.sha_file(proof_path)


def test_spec_evidence_content_is_removed_receipts_are_frozen(tmp_path):
    spec_path = tmp_path / "spec.json"
    spec = _g0_spec()
    spec["evidence"] = {"pair": {"same_files": True}}
    spec_path.write_text(json.dumps(spec), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="spec.evidence was removed"):
        pairrun.load_spec(spec_path)
    results = tmp_path / "py-results.json"
    results.write_text('{"command": "x", "selected": 1}', encoding="utf-8")
    frozen = pairrun.freeze_receipts(
        {"receipts": {"contract_python_results": str(results)}}, tmp_path / "stage"
    )
    assert frozen == {
        "contract_python_results": str(
            tmp_path / "stage" / "receipts" / "contract_python_results.json"
        )
    }
    assert (
        tmp_path / "stage" / "receipts" / "contract_python_results.json"
    ).read_bytes() == results.read_bytes()
    with pytest.raises(pairrun.RunError, match="cannot freeze receipt"):
        pairrun.freeze_receipts(
            {"receipts": {"contract_python_results": str(tmp_path / "absent.json")}},
            tmp_path / "stage2",
        )


def test_normalize_record_proves_spans_and_order(tmp_path):
    repo, suite, _run, suite_path, _rp, files = fixture_v3(tmp_path, answerable_only=True)
    _, pack, _ = ev.validate_suite(repo, suite)
    pack_sha = ev.digest(ev.canonical(pack))
    contract = pack["comparison_contract"]
    file_shas = {name: ev.digest(data) for name, data in files.items()}
    file_lines = {name: data.splitlines(keepends=True) for name, data in files.items()}

    def build(native_rows, latencies, pack_arg=pack, contract_arg=contract):
        return semble_adapter.normalize_record(
            pack_arg,
            pack_sha,
            native_rows,
            latencies,
            repo,
            file_shas,
            file_lines,
            contract_arg,
            "run-1",
            "attested",
            "method",
            "log",
            "minishlab/potion-code-16M-v2",
            "rev",
            "semble-hybrid",
            "cap-1",
            _fake_sha("diff"),
            _fake_sha("worker"),
        )

    native = [
        {
            "task_id": "T1",
            "results": [
                {"file_path": "a.txt", "start_line": 2, "end_line": 2, "score": 0.9},
                {"file_path": "b.txt", "start_line": 1, "end_line": 1, "score": 0.1},
            ],
        },
        {"task_id": "T2", "results": []},
    ]
    record = build(native, {"T1": [3.0], "T2": [1.0]})
    assert record["schema_version"] == 3
    assert [row["status"] for row in record["results"]] == ["success", "abstained"]
    assert [c["rank"] for c in record["results"][0]["candidates"]] == [1, 2]
    assert record["results"][0]["candidates"][0]["path"] == "a.txt"
    capture = record["captures"]["cap-1"]
    assert capture["system"] == "semble"
    assert capture["chunk_strategy"] == "semble_native"
    assert capture["searchd_binary"] is None and capture["generation"] == 0
    assert record["route_provenance"] == {"semble-hybrid": {"capture_id": "cap-1"}}

    drifted = build(
        [
            {
                "task_id": "T1",
                "results": [
                    {"file_path": "elsewhere/a.txt", "start_line": 1, "end_line": 1, "score": 1.0}
                ],
            }
        ],
        {"T1": [1.0]},
    )
    assert drifted["results"][0]["status"] == "error"
    assert drifted["results"][0]["error"]["code"] == "semble_hit_outside_universe"

    truncated = build(
        [
            {
                "task_id": "T1",
                "results": [{"file_path": "a.txt", "start_line": 1, "end_line": 99, "score": 1.0}],
            }
        ],
        {"T1": [1.0]},
    )
    assert truncated["results"][0]["status"] == "error"
    assert truncated["results"][0]["error"]["code"] == "semble_hit_beyond_eof"

    bad_span = build(
        [
            {
                "task_id": "T1",
                "results": [{"file_path": "a.txt", "start_line": 2, "end_line": 1, "score": 1.0}],
            }
        ],
        {"T1": [1.0]},
    )
    assert bad_span["results"][0]["status"] == "error"
    assert bad_span["results"][0]["error"]["code"] == "semble_hit_bad_span"

    with pytest.raises(semble_adapter.AdapterError, match="duplicate native row"):
        build(native + [dict(native[0])], {"T1": [1.0], "T2": [1.0]})

    over = [
        {
            "task_id": "T1",
            "results": [
                {"file_path": "a.txt", "start_line": 1, "end_line": 1, "score": 1.0},
                {"file_path": "a.txt", "start_line": 2, "end_line": 2, "score": 0.5},
            ],
        }
    ]
    narrow = dict(contract, top_k=1)
    with pytest.raises(semble_adapter.AdapterError, match="exceeded top_k"):
        build(
            over,
            {"T1": [1.0]},
            pack_arg=dict(pack, comparison_contract=narrow),
            contract_arg=narrow,
        )

    with pytest.raises(semble_adapter.AdapterError, match="pack comparison contract differs"):
        build(native, {"T1": [3.0], "T2": [1.0]}, contract_arg=dict(contract, top_k=5))

    missing = build(
        [],
        {},
        pack_arg={
            "tasks": [{"task_id": "T9", "query": "q", "query_sha256": "s"}],
            "comparison_contract": contract,
        },
    )
    assert missing["results"][0]["status"] == "error"
    assert missing["results"][0]["error"]["code"] == "semble_missing_query"
    assert missing["results"][0]["timings"]["query_latency_ms"] is None

    # Missing samples are null, never 0.
    unmeasured = build(native, {})
    assert unmeasured["results"][0]["timings"]["query_latency_ms"] is None


def _single_route_record_v3(pack_sha, contract, route, system, capture_id, rows):
    return {
        "schema_version": 3,
        "query_pack_sha256": pack_sha,
        "comparison_contract": contract,
        "runner": {
            "name": f"{system}-runner",
            "revision": "r",
            "run_id": f"run-{capture_id}",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": "attested",
            "isolation_method": "m",
            "access_block_log": "l",
        },
        "captures": {capture_id: _v3_capture(system)},
        "route_provenance": {route: {"capture_id": capture_id}},
        "results": rows,
    }


def _merge_fixture_v3(tmp_path):
    repo, suite, _run, suite_path, _rp, files = fixture_v3(tmp_path, answerable_only=True)
    suite["routes"] = ["lexical", "semble-hybrid"]
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    _, pack, _ = ev.validate_suite(repo, suite)

    def row(task_id, route, spans):
        return {
            "task_id": task_id,
            "route": route,
            "status": "success",
            "candidates": [
                _v3_block(files, *span, tokens=True, rank=i + 1) for i, span in enumerate(spans)
            ],
            "timings": {"query_latency_ms": 2.0},
            "error": None,
        }

    lex_pack, _ = pairrun.project_pack_and_suite(pack, suite, ["lexical"])
    sem_pack, _ = pairrun.project_pack_and_suite(pack, suite, ["semble-hybrid"])
    lex = _single_route_record_v3(
        ev.digest(ev.canonical(lex_pack)),
        pack["comparison_contract"],
        "lexical",
        "quanta",
        "q-lex",
        [row("T1", "lexical", [("a.txt", 2, 2)]), row("T2", "lexical", [("a.txt", 3, 3)])],
    )
    sem = _single_route_record_v3(
        ev.digest(ev.canonical(sem_pack)),
        pack["comparison_contract"],
        "semble-hybrid",
        "semble",
        "s-sem",
        [
            row("T1", "semble-hybrid", [("b.txt", 1, 1)]),
            row("T2", "semble-hybrid", [("a.txt", 4, 4)]),
        ],
    )
    lex_path = tmp_path / "lex.json"
    sem_path = tmp_path / "sem.json"
    lex_path.write_text(json.dumps(lex), encoding="utf-8")
    sem_path.write_text(json.dumps(sem), encoding="utf-8")
    return repo, suite, pack, suite_path, lex_path, sem_path, files


def test_merge_combines_disjoint_records_and_scores(tmp_path):
    repo, suite, pack, suite_path, lex_path, sem_path, files = _merge_fixture_v3(tmp_path)
    merged_suite, merged_pack, combined = pairrun.merge_records(
        repo, suite_path, [lex_path, sem_path]
    )
    assert combined["schema_version"] == 3
    assert sorted(combined["route_provenance"]) == ["lexical", "semble-hybrid"]
    assert sorted(combined["captures"]) == ["q-lex", "s-sem"]
    assert combined["comparison_contract"] == pack["comparison_contract"]
    assert len(combined["results"]) == 4
    report = ev.evaluate(merged_suite, merged_pack, combined, "semble-hybrid", "lexical")
    assert report["rank_metrics"]["comparison"]["sample_count"] == 2

    # Deterministic under input order.
    _, _, swapped = pairrun.merge_records(repo, suite_path, [sem_path, lex_path])
    assert ev.digest(ev.canonical(swapped)) == ev.digest(ev.canonical(combined))

    # Duplicate route refused (distinct captures, same route).
    lex2 = json.loads(lex_path.read_text(encoding="utf-8"))
    lex2["captures"] = {"q-lex-2": _v3_capture("quanta")}
    lex2["route_provenance"] = {"lexical": {"capture_id": "q-lex-2"}}
    lex2_path = tmp_path / "lex2.json"
    lex2_path.write_text(json.dumps(lex2), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="recorded twice"):
        pairrun.merge_records(repo, suite_path, [lex_path, lex2_path])


def test_v3_rescore_is_deterministic_under_row_order(tmp_path):
    # T13: scores never change with row order. (The merged record id still
    # commits to the exact input bytes; only scores are compared.)
    repo, suite, pack, suite_path, lex_path, sem_path, _files = _merge_fixture_v3(tmp_path)
    merged_suite, merged_pack, combined = pairrun.merge_records(
        repo, suite_path, [lex_path, sem_path]
    )
    first = ev.evaluate(merged_suite, merged_pack, combined, "semble-hybrid", "lexical")
    for path in (lex_path, sem_path):
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["results"] = list(reversed(payload["results"]))
        path.write_text(json.dumps(payload), encoding="utf-8")
    _, _, swapped = pairrun.merge_records(repo, suite_path, [sem_path, lex_path])
    second = ev.evaluate(merged_suite, merged_pack, swapped, "semble-hybrid", "lexical")
    assert ev.canonical(first["budgets"]) == ev.canonical(second["budgets"])
    assert ev.canonical(first["rank_metrics"]) == ev.canonical(second["rank_metrics"])
    assert ev.canonical(first["per_query"]) == ev.canonical(second["per_query"])

    # Capture-id collision refused.
    sem2 = json.loads(sem_path.read_text(encoding="utf-8"))
    sem2["captures"] = {"q-lex": _v3_capture("semble")}
    sem2["route_provenance"] = {"semble-hybrid": {"capture_id": "q-lex"}}
    sem2_path = tmp_path / "sem2.json"
    sem2_path.write_text(json.dumps(sem2), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="capture_id .* recorded twice"):
        pairrun.merge_records(repo, suite_path, [lex_path, sem2_path])

    # Union must cover the suite.
    with pytest.raises(pairrun.RunError, match="!= suite routes"):
        pairrun.merge_records(repo, suite_path, [lex_path])

    # Tampered pack binding refused: routes doctored after signing.
    tampered = json.loads(lex_path.read_text(encoding="utf-8"))
    tampered["route_provenance"]["semble-hybrid"] = {"capture_id": "q-lex"}
    tampered_path = tmp_path / "tampered.json"
    tampered_path.write_text(json.dumps(tampered), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="projected pack"):
        pairrun.merge_records(repo, suite_path, [tampered_path, sem_path])

    # Old artifact stamps are refused before merge.
    old_run = json.loads(lex_path.read_text(encoding="utf-8"))
    old_run["schema_version"] = 2
    run2_path = tmp_path / "old-run.json"
    run2_path.write_text(json.dumps(old_run), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="v3 record required"):
        pairrun.merge_records(repo, suite_path, [run2_path, sem_path])

    # Record/pack contract drift refused at merge.
    drifted = json.loads(lex_path.read_text(encoding="utf-8"))
    drifted["comparison_contract"]["top_k"] = 5
    drifted_path = tmp_path / "drifted.json"
    drifted_path.write_text(json.dumps(drifted), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="comparison contract differs"):
        pairrun.merge_records(repo, suite_path, [drifted_path, sem_path])


# --- Item 2: verdict state machine over frozen artifacts ---


def _counts_results(command, selected=10, executed=10, passed=10, failed=0):
    return {
        "command": command,
        "selected": selected,
        "executed": executed,
        "passed": passed,
        "failed": failed,
    }


def _receipt(command, results_bytes, revision, rail, raw_inputs):
    authority_path = (
        Path(__file__).resolve().parents[3] / "benchmarks/retrieval/proof-required-tests.json"
    )
    closure_core = {
        "schema_version": 1,
        "profile": "retrieval",
        "revision": revision,
        "roots": ["Cargo.toml"],
        "files": [
            {"path": "Cargo.toml", "sha256": _fake_sha("source")},
            {
                "path": "benchmarks/retrieval/proof-required-tests.json",
                "sha256": pairrun.sha_file(authority_path),
            },
        ],
    }
    closure = {**closure_core, "digest": ev.digest(ev.canonical(closure_core))}
    return {
        "schema_version": 2,
        "revision": revision,
        "rail": rail,
        "tier": "correctness",
        "command": command,
        "evidence_path": "results.json",
        "evidence_sha256": ev.digest(results_bytes),
        "test_event_count": 10,
        "source_closure": closure,
        "input_evidence": sorted(
            ({"role": role, "sha256": ev.digest(content)} for role, content in raw_inputs.items()),
            key=lambda entry: entry["role"],
        ),
    }


@pytest.mark.parametrize(
    "mutation",
    [
        lambda closure: closure.update(profile="other"),
        lambda closure: closure.update(revision="short"),
        lambda closure: closure.update(roots=[]),
        lambda closure: closure.update(roots=["Cargo.toml", "Cargo.toml"]),
        lambda closure: closure.update(roots=[1]),
        lambda closure: closure.update(files=[]),
        lambda closure: closure["files"][0].update(path=""),
        lambda closure: closure["files"][0].update(sha256="0"),
        lambda closure: closure["files"].append(dict(closure["files"][0])),
        lambda closure: closure.update(digest=_fake_sha("forged-closure")),
    ],
)
def test_driver_source_closure_shape_rejects_malformed_authority(mutation):
    closure = _receipt("cmd", b"{}", "a" * 40, "rail", {"raw": b"raw"})["source_closure"]
    mutation(closure)
    with pytest.raises(pairrun.RunError):
        pairrun._validate_source_closure_shape(closure, "driver source closure")


def _sdk_results(command, binary_digest, selected=6):
    return {
        "command": command,
        "separate_process": True,
        "sealed_receipt_digest": _fake_sha("sealed"),
        "activation_ack_digest": _fake_sha("ack"),
        "empty_check": True,
        "binary_digest": binary_digest,
        "sdk_route": "lexical",
        "selected": selected,
        "executed": selected,
        "passed": selected,
        "failed": 0,
    }


def _nextest_evidence(identities):
    by_binary = {}
    for identity in identities:
        binary_id, test_name = identity.split("$", 1)
        by_binary.setdefault(binary_id, []).append(test_name)
    suites = {}
    rows = []
    for binary_id, names in sorted(by_binary.items()):
        package, binary = binary_id.split("::", 1)
        kind = "lib" if binary == "quanta_index_retrieval_bench" else "test"
        metadata = {"crate": package, "test_binary": binary, "kind": kind}
        suites[binary_id] = {
            "package-name": package,
            "binary-name": binary,
            "kind": kind,
            "status": "listed",
            "testcases": {
                name: {"filter-match": {"status": "matches"}, "ignored": False} for name in names
            },
        }
        rows.append(
            {"type": "suite", "event": "started", "test_count": len(names), "nextest": metadata}
        )
        for name in names:
            event_name = f"{binary_id}${name}"
            rows.append({"type": "test", "event": "started", "name": event_name})
            rows.append({"type": "test", "event": "ok", "name": event_name})
        rows.append(
            {
                "type": "suite",
                "event": "ok",
                "passed": len(names),
                "failed": 0,
                "ignored": 0,
                "nextest": metadata,
            }
        )
    inventory = json.dumps({"test-count": len(identities), "rust-suites": suites}).encode()
    raw = b"".join(json.dumps(row).encode() + b"\n" for row in rows)
    return inventory, raw


def _sdk_raw_record(binary_digest):
    return json.dumps(
        {
            "schema_version": 3,
            "captures": {
                "capture": {
                    "runner_binary": {"digest": binary_digest},
                    "receipt_digest": _fake_sha("sealed"),
                    "activation_digest": _fake_sha("ack"),
                }
            },
            "route_provenance": {"lexical": {"capture_id": "capture"}},
        }
    ).encode()


def _parity_results(command, status="pass", failed=0):
    passed = 4 - failed
    return {
        "command": command,
        "status": status,
        "selected": 4,
        "executed": 4,
        "passed": passed,
        "failed": failed,
    }


def _full_receipts(commit, binary_digest):
    py_cmd = "python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py -q"
    rs_cmd = (
        "./scripts/cargow nextest run -p quanta-index-retrieval-bench "
        "--lib --test chunking_contract --all-features --locked"
    )
    sdk_cmd = "just retrieval-sdk-proof"
    authority = json.loads(
        (
            Path(__file__).resolve().parents[3] / "benchmarks/retrieval/proof-required-tests.json"
        ).read_text()
    )
    py_count = len(authority["python"])
    rs_count = len(authority["rust"])
    sdk_count = len(authority["sdk"])
    py_results = _counts_results(py_cmd, py_count, py_count, py_count, 0)
    rs_results = _counts_results(rs_cmd, rs_count, rs_count, rs_count, 0)
    sdk_results = _sdk_results(sdk_cmd, binary_digest, sdk_count)
    py_bytes = json.dumps(py_results).encode()
    rs_bytes = json.dumps(rs_results).encode()
    sdk_bytes = json.dumps(sdk_results).encode()
    classname = "tools.ci.tests.test_retrieval_benchmark"
    py_cases = "".join(
        f'<testcase classname="{classname}" name="{html.escape(identity[len(classname) + 1 :], quote=True)}"/>'
        for identity in authority["python"]
    )
    py_raw = (
        f'<testsuite tests="{py_count}" failures="0" errors="0" skipped="0">{py_cases}</testsuite>\n'
    ).encode()
    rust_inventory, rust_raw = _nextest_evidence(authority["rust"])
    sdk_inventory, sdk_nextest = _nextest_evidence(authority["sdk"])
    sdk_record = _sdk_raw_record(binary_digest)
    py_inventory = json.dumps(
        {
            "schema_version": 1,
            "kind": "pytest",
            "selector": "tools/ci/tests/test_retrieval_benchmark.py",
            "tests": authority["python"],
        }
    ).encode()
    return {
        "contract_python_results": py_bytes,
        "contract_python_raw": py_raw,
        "contract_python_inventory": py_inventory,
        "contract_python_receipt": _receipt(
            py_cmd,
            py_bytes,
            commit,
            "retrieval-contract-python",
            {"pytest-junit": py_raw, "pytest-inventory": py_inventory},
        ),
        "contract_rust_results": rs_bytes,
        "contract_rust_raw": rust_raw,
        "contract_rust_inventory": rust_inventory,
        "contract_rust_receipt": _receipt(
            rs_cmd,
            rs_bytes,
            commit,
            "retrieval-contract-rust",
            {"nextest-jsonl": rust_raw, "nextest-inventory": rust_inventory},
        ),
        "sdk_results": sdk_bytes,
        "sdk_nextest_raw": sdk_nextest,
        "sdk_record_raw": sdk_record,
        "sdk_inventory": sdk_inventory,
        "sdk_receipt": _receipt(
            sdk_cmd,
            sdk_bytes,
            commit,
            "retrieval-sdk-proof",
            {
                "nextest-jsonl": sdk_nextest,
                "runner-record": sdk_record,
                "nextest-inventory": sdk_inventory,
            },
        ),
    }


def _pair_stage(
    tmp_path,
    *,
    repetitions=1,
    qualified_speed_sample=False,
    blinding="attested",
    scope="exploratory",
    claims=None,
    receipts=None,
    host_clean=True,
    graded=True,
    embedder="potion-code",
    cache_regime="true_process_cold",
):
    """Build a complete valid pair stage through the real driver functions."""
    work = tmp_path / "work"
    repo, suite, run, _sp, _rp, files = fixture_v3(work / "src", answerable_only=True)
    if qualified_speed_sample:
        if repetitions != 5:
            raise ValueError("qualified speed fixture requires five fresh roots")
        original_tasks = list(suite["tasks"])
        original_results = list(run["results"])
        for index in range(3, 21):
            source_task = original_tasks[(index - 3) % len(original_tasks)]
            task = json.loads(json.dumps(source_task))
            task["task_id"] = f"T{index}"
            task["query"] = f"locate fixture {ev.digest(f'qualified-speed-task-{index}'.encode())}"
            task["query_sha256"] = ev.digest(task["query"].encode())
            task["query_family_id"] = f"fam-speed-{index}"
            suite["tasks"].append(task)
            for source_row in original_results:
                if source_row["task_id"] == source_task["task_id"]:
                    row = json.loads(json.dumps(source_row))
                    row["task_id"] = task["task_id"]
                    run["results"].append(row)
    if not graded:
        for task in suite["tasks"]:
            for label in task["gold"]:
                label.pop("grade", None)
    stage = work / "stage"
    stage.mkdir(parents=True)
    suite_path = stage / "evaluator-only" / "suite.json"
    suite_path.parent.mkdir()
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    _suite, pack, _source = ev.validate_suite(repo, suite)
    pack_path = stage / "query-pack.json"
    pack_path.write_text(json.dumps(pack), encoding="utf-8")
    corpus = {
        "repository_commit": suite["repository_commit"],
        "files": [
            {"path": "a.txt", "file_sha256": ev.digest(files["a.txt"])},
            {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"])},
        ],
    }
    corpus_path = stage / "corpus-manifest.json"
    corpus_path.write_text(json.dumps(corpus), encoding="utf-8")
    runner_corpus = stage / "runner-corpus"
    runner_corpus.mkdir()
    for name in ("a.txt", "b.txt"):
        (runner_corpus / name).write_bytes(files[name])
    corpus_view = pairrun._verify_materialized_corpus(runner_corpus, corpus_path)
    isolation_method = "m"
    access_block_log = "l"
    resource_isolation = None
    if blinding == "isolated":
        if not pairrun.SANDBOX_EXEC.is_file():
            pytest.skip("macOS Seatbelt backend is unavailable")
        secret_root = work / "evaluator-secret"
        secret_root.mkdir()
        denied_roots = sorted(
            {str(secret_root.resolve()), str(suite_path.parent.resolve()), str(repo.resolve())}
        )
        allowed_read_roots = sorted(
            {str(pack_path.resolve()), str(runner_corpus.resolve()), str(stage.resolve())}
        )
        allowed_write_roots = [str(stage.resolve())]
        runner_tools = stage / "runner-tools"
        runner_tools.mkdir()
        for name in ("evaluator.py", "semble.py"):
            (runner_tools / name).write_text(name, encoding="utf-8")
        profile = pairrun._seatbelt_profile(denied_roots, allowed_read_roots, allowed_write_roots)
        proof = {
            "schema_version": 1,
            "backend": pairrun.ISOLATION_BACKEND,
            "sandbox_exec": {
                "path": str(pairrun.SANDBOX_EXEC),
                "sha256": pairrun.sha_file(pairrun.SANDBOX_EXEC),
            },
            "profile_sha256": hashlib.sha256(profile.encode()).hexdigest(),
            "denied_roots": denied_roots,
            "allowed_read_roots": allowed_read_roots,
            "allowed_write_roots": allowed_write_roots,
            "suite": {
                "path": "evaluator-only/suite.json",
                "capture_path": str(suite_path.resolve()),
                "sha256": pairrun.sha_file(suite_path),
            },
            "query_pack": {
                "path": "query-pack.json",
                "capture_path": str(pack_path.resolve()),
                "sha256": pairrun.sha_file(pack_path),
            },
            "corpus_view": {
                "path": "runner-corpus",
                "manifest_sha256": corpus_view["manifest_sha256"],
                "proof_sha256": corpus_view["proof_sha256"],
                "file_count": len(corpus_view["files"]),
            },
            "runner_tools": [
                {"path": f"runner-tools/{name}", "sha256": pairrun.sha_file(runner_tools / name)}
                for name in ("evaluator.py", "semble.py")
            ],
            "probes": {
                "suite_read_denied": True,
                "query_pack_read_allowed": True,
            },
        }
        proof_path = stage / "isolation-proof.json"
        proof_path.write_text(json.dumps(proof), encoding="utf-8")
        proof_sha = pairrun.sha_file(proof_path)
        isolation_method = pairrun.ISOLATION_BACKEND
        access_block_log = f"sha256:{proof_sha}"
        resource_isolation = {
            "backend": pairrun.ISOLATION_BACKEND,
            "profile_sha256": proof["profile_sha256"],
            "proof_sha256": proof_sha,
        }
    corpus_dir = work / "corpus"
    corpus_dir.mkdir(parents=True)
    for name in ("a.txt", "b.txt"):
        (corpus_dir / name).write_bytes(files[name])
    admitted = [(e["path"], e["file_sha256"]) for e in corpus["files"]]
    mapping, _diff = semble_adapter.mapping_proof(admitted, ["a.txt", "b.txt"], corpus_dir)
    binary_digest = ev.digest(b"quanta-runner-binary")
    runner_binary = work / "runner-bin"
    runner_binary.write_bytes(b"quanta-runner-binary")

    lex_pack, _ = pairrun.project_pack_and_suite(pack, suite, ["lexical"])
    sem_pack, _ = pairrun.project_pack_and_suite(pack, suite, ["hybrid"])
    lex_sha = ev.digest(ev.canonical(lex_pack))
    sem_sha = ev.digest(ev.canonical(sem_pack))
    lex_rows = [r for r in run["results"] if r["route"] == "lexical"]
    sem_rows = [r for r in run["results"] if r["route"] == "hybrid"]

    def record(route_rows, pack_sha, system, capture_id, run_id):
        capture = _v3_capture(system)
        if system == "quanta":
            capture["runner_binary"]["digest"] = binary_digest
        else:
            capture["receipt_digest"] = mapping["diff_digest"]
        return {
            "schema_version": 3,
            "query_pack_sha256": pack_sha,
            "comparison_contract": pack["comparison_contract"],
            "runner": {
                "name": f"{system}-runner",
                "revision": "r",
                "run_id": run_id,
                "tokenizer": ev.TOKENIZER,
                "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
                "gold_access": False,
                "blinding": blinding,
                "isolation_method": isolation_method,
                "access_block_log": access_block_log,
            },
            "captures": {capture_id: capture},
            "route_provenance": {route_rows[0]["route"]: {"capture_id": capture_id}},
            "results": json.loads(json.dumps(route_rows)),
        }

    rep_layouts = []
    measurements = 10 if qualified_speed_sample else 1
    warm_query_ms = len(pack["tasks"]) * measurements * 1.5 if qualified_speed_sample else 3.0
    for rep in range(repetitions):
        rep_dir = stage / f"rep-{rep:02d}"
        qdir = rep_dir / "quanta" / "strategy-00-whole_file"
        sdir = rep_dir / "semble"
        qdir.mkdir(parents=True)
        sdir.mkdir(parents=True)
        qrec = record(lex_rows, lex_sha, "quanta", f"q-r{rep}", f"run-q-r{rep}")
        srec = record(sem_rows, sem_sha, "semble", f"s-r{rep}", f"run-s-r{rep}")
        qpath = qdir / "record.json"
        spath = sdir / "record.json"
        qpath.write_text(json.dumps(qrec), encoding="utf-8")
        spath.write_text(json.dumps(srec), encoding="utf-8")
        task_ids = [task["task_id"] for task in pack["tasks"]]
        protocol = pairrun.build_query_protocol(task_ids, rep, 1, measurements)
        protocol_path = rep_dir / "query-protocol.json"
        protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
        q_warm = {
            "lexical": {
                row["task_id"]: [row["timings"]["query_latency_ms"]] * measurements
                for row in qrec["results"]
            }
        }
        s_warm = {
            "hybrid": {
                row["task_id"]: [row["timings"]["query_latency_ms"]] * measurements
                for row in srec["results"]
            }
        }
        qphase = qdir / "phase-metrics.json"
        qphase.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "system": "quanta",
                    "timing_layer": "runner_monotonic_wall_v1",
                    "strategy": "whole_file",
                    "record_sha256": ev.digest(qpath.read_bytes()),
                    "runner_binary_sha256": binary_digest,
                    "task_count": len(pack["tasks"]),
                    "route_count": 1,
                    "file_count": 2,
                    "chunk_count": 2,
                    "query_schedule": [task["task_id"] for task in pack["tasks"]],
                    "warmup_passes": 1,
                    "measurement_repetitions": measurements,
                    "query_protocol": protocol,
                    "warm_latencies_ms": q_warm,
                    "cold_latencies_ms": {"lexical": 1.0},
                    "phases_ms": {
                        "discovery": 1.0,
                        "chunk": 1.0,
                        "model_provider_prepare": 1.0,
                        "embed_publish_seal_activate": 1.0,
                        "cold_query": 1.0,
                        "warmup": 1.0,
                        "warm_query": warm_query_ms,
                        "unattributed": 1.0,
                    },
                    "total_ms": 7.0 + warm_query_ms,
                }
            ),
            encoding="utf-8",
        )
        sphase = sdir / "phase-metrics.json"
        sphase.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "system": "semble",
                    "timing_layer": "worker_monotonic_wall_v1",
                    "strategy": "native",
                    "record_sha256": ev.digest(spath.read_bytes()),
                    "worker_sha256": _fake_sha("worker"),
                    "task_count": len(pack["tasks"]),
                    "route_count": 1,
                    "file_count": 2,
                    "chunk_count": 2,
                    "query_schedule": [task["task_id"] for task in pack["tasks"]],
                    "warmup_passes": 1,
                    "measurement_repetitions": measurements,
                    "query_protocol": protocol,
                    "warm_latencies_ms": s_warm,
                    "cold_latencies_ms": {"hybrid": 1.0},
                    "phases_ms": {
                        "discovery": 1.0,
                        "model_provider_prepare": 1.0,
                        "index": 1.0,
                        "warmup": 1.0,
                        "cold_query": 1.0,
                        "warm_query": warm_query_ms,
                        "unattributed": 1.0,
                    },
                    "phase_boundaries_ns": {
                        "worker_start": 0,
                        "discovery_end": 1_000_000,
                        "model_provider_prepare_end": 2_000_000,
                        "index_end": 3_000_000,
                        "cold_query_start": 3_000_000,
                        "cold_query_end": 4_000_000,
                        "warmup_end": 5_000_000,
                        "query_start": 5_000_000,
                        "first_query_start": 5_000_000,
                        "first_query_end": 6_500_000,
                        "query_end": 5_000_000 + int(warm_query_ms * 1_000_000),
                        "worker_end": 6_000_000 + int(warm_query_ms * 1_000_000),
                    },
                    "total_ms": 6.0 + warm_query_ms,
                }
            ),
            encoding="utf-8",
        )
        resource_payload = {
            "schema_version": 1,
            "sampler": "ps-process-tree-rss-cpu-v2",
            "sample_interval_ms": 50,
            "command_sha256": _fake_sha("command"),
            "root_pid": 100 + rep,
            "exit_code": 0,
            "timed_out": False,
            "elapsed_ms": 7.0 + warm_query_ms,
            "peak_rss_bytes": 4096,
            "peak_cpu_percent": 10.0,
            "processes": [
                {
                    "pid": 100 + rep,
                    "command": "runner",
                    "peak_rss_bytes": 4096,
                    "peak_cpu_percent": 10.0,
                    "samples": 2,
                }
            ],
            "storage": {
                "index_bytes": 4096,
                "model_cache_bytes": 1024,
                "parser_cache_bytes": 0,
                "embedding_cache_bytes": 0,
                "discovered_files": 2,
                "indexed_chunks": 2,
                "index_storage": "disk",
                "index_measurement": "filesystem_tree_v1",
            },
            "samples": 2,
            "complete": True,
            "error": None,
            "cleanup_complete": True,
            "cleanup_escalated": False,
            "cleanup_error": None,
        }
        if resource_isolation is not None:
            resource_payload["isolation"] = resource_isolation
        qresource = qdir / "resource-metrics.json"
        qresource.write_text(
            json.dumps({**resource_payload, "subject_sha256": ev.digest(qpath.read_bytes())}),
            encoding="utf-8",
        )
        sresource = rep_dir / "semble-resource-metrics.json"
        sresource.write_text(
            json.dumps(
                {
                    **resource_payload,
                    "subject_sha256": ev.digest(spath.read_bytes()),
                    "storage": {
                        **resource_payload["storage"],
                        "index_bytes": 4096,
                        "index_storage": "memory",
                        "index_measurement": "process_peak_rss_delta_v1",
                    },
                }
            ),
            encoding="utf-8",
        )
        latencies = {
            row["task_id"]: [
                row["timings"]["query_latency_ms"],
                row["timings"]["query_latency_ms"] + 0.1,
            ]
            for row in srec["results"]
        }
        (sdir / "native.json").write_text(
            json.dumps(
                {
                    "native": [
                        {"task_id": row["task_id"], "results": []} for row in srec["results"]
                    ],
                    "latencies_ms": latencies,
                    "stats": {
                        "indexed_files": 2,
                        "total_chunks": 2,
                        "index_resident_bytes": 4096,
                        "index_measurement": "process_peak_rss_delta_v1",
                    },
                }
            ),
            encoding="utf-8",
        )
        (sdir / "mapping-proof.json").write_text(json.dumps(mapping), encoding="utf-8")
        lockfile = sdir / "lockfile.txt"
        lockfile.write_bytes(b"semble==0.6.0\n")
        (sdir / "adapter-manifest.json").write_text(
            json.dumps(
                {
                    "semble_version": "0.6.0",
                    "semble_python": "/venv/bin/python",
                    "interpreter": {
                        "path": "/venv/bin/python",
                        "realpath": "/venv/bin/python3.11",
                        "version": "3.11",
                        "digest": _fake_sha("interp"),
                    },
                    "worker_digest": _fake_sha("worker"),
                    "lockfile_digest": ev.digest(b"semble==0.6.0\n"),
                    "model_id": "m",
                    "model_revision": "r",
                    "model_asset_digest": _fake_sha("model"),
                    "record_digest": ev.digest(spath.read_bytes()),
                }
            ),
            encoding="utf-8",
        )
        (rep_dir / "quanta" / "quanta-manifest.json").write_text(
            json.dumps(
                {
                    "runs": [
                        {
                            "strategy": "whole_file",
                            "record": "strategy-00-whole_file/record.json",
                            "record_digest": ev.digest(qpath.read_bytes()),
                            "index_bytes": 4096,
                            "runner_binary_sha256": binary_digest,
                            "driver_ms": 1.0,
                            "phase_metrics": "strategy-00-whole_file/phase-metrics.json",
                            "phase_metrics_digest": ev.digest(qphase.read_bytes()),
                            "resource_metrics": "strategy-00-whole_file/resource-metrics.json",
                            "resource_metrics_digest": ev.digest(qresource.read_bytes()),
                            "state_root": "strategy-00-whole_file/state",
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )
        rep_layouts.append(
            {
                "rep": rep,
                "order": ["quanta", "semble"],
                "query_protocol": str(protocol_path),
                "quanta": {"whole_file": str(qpath)},
                "quanta_phase_metrics": {"whole_file": str(qphase)},
                "semble": str(spath),
                "quanta_manifest": str(rep_dir / "quanta" / "quanta-manifest.json"),
                "semble_phase_metrics": str(sphase),
                "semble_resource_metrics": str(sresource),
            }
        )

    matrix = pairrun.build_latency_matrix(rep_layouts)
    (stage / "latency-matrix.json").write_text(json.dumps(matrix), encoding="utf-8")
    _s, _p, combined = pairrun.merge_records(
        repo,
        suite_path,
        [Path(rep_layouts[0]["quanta"]["whole_file"]), Path(rep_layouts[0]["semble"])],
    )
    report = ev.evaluate(_s, _p, combined, "hybrid", "lexical")
    report_name = "report-hybrid-vs-lexical-whole_file.json"
    (stage / report_name).write_text(json.dumps(report), encoding="utf-8")
    host = {
        "system": "Darwin",
        "release": "test",
        "machine": "arm64",
        "processor": "test-cpu",
        "cpu_count": 8,
        "python": "3.11",
        "rustc": "rustc test",
        "concurrent_processes": {"none": []},
        "contention_override": False,
        "thermal": {"status": "clean", "evidence": "test"},
        "frequency": {"status": "bounded", "evidence": {}},
        "power": {"status": "bounded", "digest": _fake_sha("power")},
    }
    if not host_clean:
        host = {**host, "concurrent_processes": {"cargo": [123]}}
    host_start = dict(host)
    host_end = dict(host)
    (stage / "host-start.json").write_text(json.dumps(host_start), encoding="utf-8")
    (stage / "host-end.json").write_text(json.dumps(host_end), encoding="utf-8")
    host_profile = stage / "host-profile.json"
    host_profile.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "profile_id": "test-host",
                "fingerprint": pairrun._host_fingerprint(host),
            }
        ),
        encoding="utf-8",
    )
    semble_lockfile = stage / "rep-00" / "semble" / "lockfile.txt"
    spec = {
        "manifest": str(corpus_path),
        "suite": str(suite_path),
        "query_pack": str(pack_path),
        "runner_binary": str(runner_binary),
        "semble_lockfile": str(semble_lockfile),
        "host_profile": str(host_profile),
        "blinding": blinding,
        "isolation_method": isolation_method,
        "access_block_log": access_block_log,
        "scope": scope,
        "claims": claims or {},
        "embedder": embedder,
        "cache_regime": cache_regime,
    }
    if receipts == "full" or (scope == "qualified" and receipts is None):
        source_sha = pairrun.git_head_sha(Path(__file__).resolve().parents[3])
        contents = _full_receipts(source_sha, binary_digest)
    else:
        contents = receipts or {}
    frozen = {}
    if contents:
        rdir = stage / "receipts"
        rdir.mkdir(exist_ok=True)
        for key, content in contents.items():
            data = content if isinstance(content, bytes) else json.dumps(content).encode()
            target = rdir / f"{key}.json"
            target.write_bytes(data)
            frozen[key] = str(target)

    frozen_admission = {}
    driver_source_closure_digest = None
    if scope == "qualified":
        source_receipt = json.loads(
            Path(frozen["contract_python_receipt"]).read_text(encoding="utf-8")
        )
        source_closure_path = stage / "driver-source-closure.json"
        source_closure_path.write_text(
            json.dumps(source_receipt["source_closure"]), encoding="utf-8"
        )
        spec["_driver_source_closure"] = str(source_closure_path)
        driver_source_closure_digest = source_receipt["source_closure"]["digest"]
        evidence_dir = stage / "admission"
        evidence_dir.mkdir(exist_ok=True)
        license_path = evidence_dir / "license-receipt.json"
        annotation_paths = [
            evidence_dir / "annotation-1-receipt.json",
            evidence_dir / "annotation-2-receipt.json",
        ]
        adjudication_path = evidence_dir / "adjudication-receipt.json"
        license_path.write_text(
            '{"decision":"approved","reviewer":"license-owner"}', encoding="utf-8"
        )
        annotation_paths[0].write_text('{"annotator":"gold-owner-a"}', encoding="utf-8")
        annotation_paths[1].write_text('{"annotator":"gold-owner-b"}', encoding="utf-8")
        adjudication_path.write_text('{"adjudicator":"gold-adjudicator"}', encoding="utf-8")
        admission_path = evidence_dir / "admission.json"
        admission = {
            "schema_version": 1,
            "admission_id": "test-qualified-admission",
            "issued_at": "2026-09-24T00:00:00Z",
            "source_revision": pairrun.git_head_sha(Path(__file__).resolve().parents[3]),
            "repository_commit": suite["repository_commit"],
            "corpus_manifest_sha256": pairrun.sha_file(corpus_path),
            "suite_sha256": pairrun.sha_file(suite_path),
            "query_pack_sha256": pairrun.sha_file(pack_path),
            "license": {
                "reviewer_id": "license-owner",
                "decision": "approved",
                "receipt_sha256": pairrun.sha_file(license_path),
            },
            "gold": {
                "frozen_before_results": True,
                "annotators": [
                    {
                        "annotator_id": "gold-owner-a",
                        "receipt_sha256": pairrun.sha_file(annotation_paths[0]),
                    },
                    {
                        "annotator_id": "gold-owner-b",
                        "receipt_sha256": pairrun.sha_file(annotation_paths[1]),
                    },
                ],
                "adjudicator_id": "gold-adjudicator",
                "adjudication_receipt_sha256": pairrun.sha_file(adjudication_path),
            },
            "models": {
                "quanta_model_revision": "r1",
                "semble_model_revision": "r",
                "semble_model_asset_sha256": _fake_sha("model"),
            },
            "semble_lockfile_sha256": pairrun.sha_file(semble_lockfile),
            "host_profile_sha256": pairrun.sha_file(host_profile),
            "cache_regime": cache_regime,
            "verification": {
                "contract_python_receipt_sha256": pairrun.sha_file(
                    Path(frozen["contract_python_receipt"])
                ),
                "contract_rust_receipt_sha256": pairrun.sha_file(
                    Path(frozen["contract_rust_receipt"])
                ),
                "sdk_receipt_sha256": pairrun.sha_file(Path(frozen["sdk_receipt"])),
            },
        }
        admission_path.write_text(json.dumps(admission), encoding="utf-8")
        frozen_admission = {
            "manifest": str(admission_path),
            "license_receipt": str(license_path),
            "annotation_receipts": [str(path) for path in annotation_paths],
            "adjudication_receipt": str(adjudication_path),
        }
        spec["admission"] = dict(frozen_admission)

    (stage / "protocol-lock.json").write_text(
        json.dumps(
            {
                "suite_digest": ev.digest(suite_path.read_bytes()),
                "query_pack_digest": ev.digest(pack_path.read_bytes()),
                "corpus_manifest_digest": ev.digest(corpus_path.read_bytes()),
                "top_k": 10,
                "strategies": ["whole_file"],
                "searchd_expected_sha256": _fake_sha("searchd"),
                "semble_lockfile_sha256": ev.digest(b"semble==0.6.0\n"),
                "host_profile_digest": pairrun.sha_file(host_profile),
                "admission_digest": (
                    pairrun.sha_file(Path(frozen_admission["manifest"]))
                    if frozen_admission
                    else None
                ),
                "driver_source_closure_digest": driver_source_closure_digest,
                "repetitions": repetitions,
                "base_seed": 0,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": measurements,
                "query_protocol_sha256s": [
                    json.loads(Path(layout["query_protocol"]).read_text(encoding="utf-8"))["sha256"]
                    for layout in rep_layouts
                ],
            }
        ),
        encoding="utf-8",
    )
    manifest = pairrun.build_run_manifest(
        spec, stage, rep_layouts, host_start, host_end, [report_name], frozen, frozen_admission
    )
    manifest_path = stage / "run-manifest.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    return {
        "repo": repo,
        "suite": suite,
        "stage": stage,
        "spec": spec,
        "manifest": manifest,
        "manifest_path": manifest_path,
        "suite_path": suite_path,
        "rep_layouts": rep_layouts,
        "binary_digest": binary_digest,
        "commit": suite["repository_commit"],
    }


def _stage_verdict(st):
    return pairrun.build_verdict(st["repo"], st["suite_path"], st["manifest_path"])


def _rewrite_manifest(st, mutator):
    manifest = json.loads(st["manifest_path"].read_text(encoding="utf-8"))
    mutator(manifest)
    st["manifest_path"].write_text(json.dumps(manifest), encoding="utf-8")


def test_verdict_pair_only_green(tmp_path):
    st = _pair_stage(tmp_path)
    jsonschema.validate(st["manifest"], _load_schema("run-manifest.schema.json"))
    verdict = _stage_verdict(st)
    jsonschema.validate(verdict, _load_schema("verdict.schema.json"))
    assert verdict["verdict_version"] == 2
    assert verdict["states"] == {
        "CONTRACT_GREEN": "not_run",
        "SDK_PATH_GREEN": "not_run",
        "PAIR_VALID": "pass",
        "PERF_QUALIFIED": "not_applicable",
        "QUALITY_DELTA": "not_applicable",
    }
    assert verdict["failure_class"] == "none"
    assert "T01" in verdict["missing_t_ids"] and "T05" in verdict["missing_t_ids"]
    assert "T00" not in verdict["missing_t_ids"]
    assert verdict["not_applicable_t_ids"] == ["T15", "T16"]
    assert len(verdict["comparisons"]) == 1
    comparison = verdict["comparisons"][0]
    assert comparison["strategy"] == "whole_file"
    assert comparison["baseline_route"] == "hybrid"
    assert comparison["candidate_route"] == "lexical"
    assert len(comparison["record_digest"]) == 64
    assert verdict["counts"] == {"selected": 4, "executed": 4, "passed": 4, "failed": 0}
    for name, proof in verdict["state_evidence"].items():
        assert proof["reason"], name
    assert verdict["state_evidence"]["PAIR_VALID"]["proof_digest"] is not None
    assert st["commit"] != st["manifest"]["provenance"]["quanta"]["source_sha"]
    assert (
        verdict["provenance"]["quanta"]["source_sha"]
        == (st["manifest"]["provenance"]["quanta"]["source_sha"])
    )


def test_verdict_incomplete_observation_fails_pair(tmp_path):
    st = _pair_stage(tmp_path)
    layout = st["rep_layouts"][0]
    spath = Path(layout["semble"])
    payload = json.loads(spath.read_text(encoding="utf-8"))
    assert payload["results"], "stage needs at least one semble row"
    payload["results"][0].update(
        status="error",
        candidates=[],
        timings={"query_latency_ms": None},
        error={"code": "semble_hit_bad_span", "message": "stub span failure"},
    )
    spath.write_text(json.dumps(payload), encoding="utf-8")
    adapter_path = spath.parent / "adapter-manifest.json"
    adapter = json.loads(adapter_path.read_text(encoding="utf-8"))
    adapter["record_digest"] = ev.digest(spath.read_bytes())
    adapter_path.write_text(json.dumps(adapter), encoding="utf-8")
    # A real run scores whatever the capture observed: rebuild the report
    # from the mutated records so only the incomplete-observation gate fires.
    _s, _p, combined = pairrun.merge_records(
        st["repo"], st["suite_path"], [Path(layout["quanta"]["whole_file"]), spath]
    )
    report = ev.evaluate(_s, _p, combined, "hybrid", "lexical")
    (st["stage"] / "report-hybrid-vs-lexical-whole_file.json").write_text(
        json.dumps(report), encoding="utf-8"
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"].startswith(
        "incomplete_observation:rep-00:semble:semble_native:"
    )
    assert verdict["counts"]["failed"] == 1


def test_verdict_full_receipts_all_green(tmp_path):
    st = _pair_stage(tmp_path, receipts="full")
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "pass"
    assert verdict["states"]["SDK_PATH_GREEN"] == "pass"
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["failure_class"] == "none"
    assert verdict["missing_t_ids"] == []


def test_verdict_lying_manifest_refused(tmp_path):
    st = _pair_stage(tmp_path, claims={"speed": True})
    # Fake contract evidence without artifacts fails instead of passing.
    _rewrite_manifest(
        st,
        lambda m: m["evidence"].update(
            {
                "contract_suites": {
                    "python": {
                        "test_result_digest": "a" * 64,
                        "raw_evidence_digest": "c" * 64,
                        "inventory_digest": "e" * 64,
                    },
                    "rust": {
                        "test_result_digest": "b" * 64,
                        "raw_evidence_digest": "d" * 64,
                        "inventory_digest": "f" * 64,
                    },
                }
            }
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert verdict["failure_class"] == "scoring"
    # Inflated perf numbers are re-derived, never trusted.
    st = _pair_stage(tmp_path / "perf", scope="qualified", claims={"speed": True})
    _rewrite_manifest(st, lambda m: m["evidence"]["perf"].update({"observations_floor": 9999}))
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "perf_floor_mismatch"
    # A swapped binary pin fails the pair binding.
    st = _pair_stage(tmp_path / "binary")
    _rewrite_manifest(st, lambda m: m["provenance"]["quanta"].update({"binary_digest": "0" * 64}))
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"] == "binary_digest_mismatch"


def test_verdict_stale_and_swapped_receipts(tmp_path):
    st = _pair_stage(tmp_path, receipts="full")
    results_path = st["stage"] / "receipts" / "contract_python_results.json"
    tampered = json.loads(results_path.read_text(encoding="utf-8"))
    tampered["passed"] = 104
    results_path.write_text(json.dumps(tampered), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert "receipt digest mismatch" in verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]

    st = _pair_stage(tmp_path / "closure", receipts="full")
    rust_receipt_path = st["stage"] / "receipts" / "contract_rust_receipt.json"
    rust_receipt = json.loads(rust_receipt_path.read_text(encoding="utf-8"))
    closure = rust_receipt["source_closure"]
    closure["files"][0]["sha256"] = _fake_sha("different-source")
    closure_core = {
        key: closure[key] for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    closure["digest"] = ev.digest(ev.canonical(closure_core))
    rust_receipt_path.write_text(json.dumps(rust_receipt), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert "different source closures" in verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]
    st = _pair_stage(tmp_path / "swap", receipts="full")
    rust_bytes = (st["stage"] / "receipts" / "contract_rust_results.json").read_bytes()
    (st["stage"] / "receipts" / "contract_python_results.json").write_bytes(rust_bytes)
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"


def test_verdict_garbage_test_artifact(tmp_path):
    st = _pair_stage(tmp_path, receipts="full")
    raw_path = st["stage"] / "receipts" / "contract_python_raw.json"
    raw_path.write_text(
        '<testsuite tests="105" failures="1" errors="0" skipped="0" />\n',
        encoding="utf-8",
    )
    raw_digest = ev.digest(raw_path.read_bytes())
    receipt_path = st["stage"] / "receipts" / "contract_python_receipt.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    next(entry for entry in receipt["input_evidence"] if entry["role"] == "pytest-junit")[
        "sha256"
    ] = raw_digest
    receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: (
            manifest["evidence"]["contract_suites"]["python"].update(
                raw_evidence_digest=raw_digest
            ),
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert "raw evidence refused" in verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]


def test_verdict_rejects_coordinated_partial_inventory_and_receipt_rebind(tmp_path):
    st = _pair_stage(tmp_path, receipts="full")
    assert _stage_verdict(st)["states"]["CONTRACT_GREEN"] == "pass"
    manifest = json.loads(st["manifest_path"].read_text())
    paths = {
        key: st["stage"] / manifest["artifacts"][f"contract_python_{key}"]
        for key in ("raw", "inventory", "results", "receipt")
    }
    inventory = json.loads(paths["inventory"].read_text())
    inventory["tests"] = inventory["tests"][:1]
    paths["inventory"].write_text(json.dumps(inventory), encoding="utf-8")
    identity = inventory["tests"][0]
    classname = "tools.ci.tests.test_retrieval_benchmark"
    paths["raw"].write_text(
        f'<testsuite tests="1" failures="0" errors="0" skipped="0">'
        f'<testcase classname="{classname}" '
        f'name="{html.escape(identity[len(classname) + 1 :], quote=True)}"/>'
        "</testsuite>",
        encoding="utf-8",
    )
    results = _counts_results(
        "python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py -q", 1, 1, 1, 0
    )
    paths["results"].write_text(json.dumps(results), encoding="utf-8")
    receipt = json.loads(paths["receipt"].read_text())
    receipt["evidence_sha256"] = pairrun.sha_file(paths["results"])
    receipt["test_event_count"] = 1
    for entry in receipt["input_evidence"]:
        if entry["role"] == "pytest-junit":
            entry["sha256"] = pairrun.sha_file(paths["raw"])
        elif entry["role"] == "pytest-inventory":
            entry["sha256"] = pairrun.sha_file(paths["inventory"])
    paths["receipt"].write_text(json.dumps(receipt), encoding="utf-8")
    manifest["evidence"]["contract_suites"]["python"].update(
        test_result_digest=pairrun.sha_file(paths["results"]),
        raw_evidence_digest=pairrun.sha_file(paths["raw"]),
        inventory_digest=pairrun.sha_file(paths["inventory"]),
    )
    st["manifest_path"].write_text(json.dumps(manifest), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["CONTRACT_GREEN"] == "fail"
    assert (
        "inventory differs from source authority"
        in verdict["state_evidence"]["CONTRACT_GREEN"]["reason"]
    )


def test_verdict_mapping_lies(tmp_path):
    st = _pair_stage(tmp_path)
    mapping_path = st["stage"] / "rep-00" / "semble" / "mapping-proof.json"
    mapping = json.loads(mapping_path.read_text(encoding="utf-8"))
    mapping["skipped"] = [{"path": "evil.txt", "reason": "policy"}]
    mapping_path.write_text(json.dumps(mapping), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert "T00" in verdict["missing_t_ids"]
    st = _pair_stage(tmp_path / "corpus")
    corpus_path = st["stage"] / "corpus-manifest.json"
    corpus = json.loads(corpus_path.read_text(encoding="utf-8"))
    corpus["files"][0]["file_sha256"] = "f" * 64
    corpus_path.write_text(json.dumps(corpus), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["failure_class"] == "corpus_mismatch"


def test_verdict_report_tamper(tmp_path):
    st = _pair_stage(tmp_path)
    report_path = st["stage"] / "report-hybrid-vs-lexical-whole_file.json"
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report["rank_metrics"]["comparison"]["primary_delta"] = 1.0
    report_path.write_text(json.dumps(report), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"] == "report_not_reproducible"
    assert "T13" in verdict["missing_t_ids"]


def test_verdict_matrix_tamper_and_native_disagreement(tmp_path):
    st = _pair_stage(tmp_path)
    matrix_path = st["stage"] / "latency-matrix.json"
    matrix = json.loads(matrix_path.read_text(encoding="utf-8"))
    matrix["observations_floor"] = 9999
    matrix_path.write_text(json.dumps(matrix), encoding="utf-8")
    verdict = _stage_verdict(st)
    # Without a speed claim the matrix is not pair evidence.
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["states"]["PERF_QUALIFIED"] == "not_applicable"
    st = _pair_stage(tmp_path / "speed", scope="qualified", claims={"speed": True})
    matrix_path = st["stage"] / "latency-matrix.json"
    matrix = json.loads(matrix_path.read_text(encoding="utf-8"))
    matrix["observations_floor"] = 9999
    matrix_path.write_text(json.dumps(matrix), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "matrix_not_reproducible"
    st = _pair_stage(tmp_path / "warm", scope="qualified", claims={"speed": True})
    phase_path = st["stage"] / "rep-00" / "semble" / "phase-metrics.json"
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    first_task = next(iter(phase["warm_latencies_ms"]["hybrid"]))
    phase["warm_latencies_ms"]["hybrid"][first_task][0] = 9.9
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "phase_boundaries_incomplete"


def test_verdict_perf_frontier_and_gates(tmp_path, monkeypatch):
    st = _pair_stage(tmp_path, claims={"speed": True})
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "not_applicable"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "exploratory_only"
    st = _pair_stage(tmp_path / "qualified", scope="qualified", claims={"speed": True})
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "observations_floor_unmet"
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 2)
    monkeypatch.setattr(pairrun, "FRESH_ROOTS_FLOOR", 1)
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert "at least 20 frozen tasks" in verdict["state_evidence"]["PERF_QUALIFIED"]["reason"]
    # Null timings fail a speed claim once floors hold.
    st = _pair_stage(tmp_path / "nulls", scope="qualified", claims={"speed": True})
    record_path = st["stage"] / "rep-00" / "quanta" / "strategy-00-whole_file" / "record.json"
    record = json.loads(record_path.read_text(encoding="utf-8"))
    record["results"][0]["timings"] = {"query_latency_ms": None}
    record_path.write_text(json.dumps(record), encoding="utf-8")
    record_digest = ev.digest(record_path.read_bytes())
    phase_path = record_path.parent / "phase-metrics.json"
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    phase["record_sha256"] = record_digest
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    resource_path = record_path.parent / "resource-metrics.json"
    resource = json.loads(resource_path.read_text(encoding="utf-8"))
    resource["subject_sha256"] = record_digest
    resource_path.write_text(json.dumps(resource), encoding="utf-8")
    matrix = pairrun.build_latency_matrix(st["rep_layouts"])
    (st["stage"] / "latency-matrix.json").write_text(json.dumps(matrix), encoding="utf-8")
    quanta_manifest_path = st["stage"] / "rep-00" / "quanta" / "quanta-manifest.json"
    quanta_manifest = json.loads(quanta_manifest_path.read_text(encoding="utf-8"))
    quanta_manifest["runs"][0].update(
        record_digest=record_digest,
        phase_metrics_digest=ev.digest(phase_path.read_bytes()),
        resource_metrics_digest=ev.digest(resource_path.read_bytes()),
    )
    quanta_manifest_path.write_text(json.dumps(quanta_manifest), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda m: m["evidence"]["perf"].update(
            {
                "observations_floor": matrix["observations_floor"],
                "fresh_roots": matrix["fresh_roots"],
            }
        ),
    )
    # Nulling a sample drops the floor to 1; hold floors there to isolate
    # the null-timing gate.
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 1)
    verdict = _stage_verdict(st)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "null_timings_on_speed_claim"
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 2)
    # Contended hosts fail with host class; the digests stay consistent.
    st = _pair_stage(tmp_path / "host", scope="qualified", claims={"speed": True})
    busy = {"concurrent_processes": {"cargo": [123]}, "contention_override": False}
    (st["stage"] / "host-start.json").write_text(json.dumps(busy), encoding="utf-8")

    def rebind(manifest):
        start = json.loads((st["stage"] / "host-start.json").read_text(encoding="utf-8"))
        end = json.loads((st["stage"] / "host-end.json").read_text(encoding="utf-8"))
        manifest["host"] = {
            "start_digest": ev.digest((st["stage"] / "host-start.json").read_bytes()),
            "end_digest": ev.digest((st["stage"] / "host-end.json").read_bytes()),
            "cache_regime": manifest["host"]["cache_regime"],
        }
        manifest["provenance"]["host"]["check_record_digest"] = ev.digest(
            ev.canonical({"start": start, "end": end})
        )

    _rewrite_manifest(st, rebind)
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "host_contended"
    assert verdict["failure_class"] == "host"


def test_qualified_speed_verdict_accepts_full_observation_protocol(tmp_path):
    st = _pair_stage(
        tmp_path,
        repetitions=5,
        qualified_speed_sample=True,
        scope="qualified",
        claims={"speed": True},
    )
    matrix = pairrun.read_json(st["stage"] / "latency-matrix.json")
    assert matrix["fresh_roots"] == 5
    assert matrix["observations_floor"] == 1_000
    verdict = _stage_verdict(st)
    # Contract receipts bind to committed source; this fixture isolates the
    # independent performance state even in a dirty edit loop.
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["states"]["PERF_QUALIFIED"] == "pass"
    assert verdict["states"]["QUALITY_DELTA"] == "not_applicable"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "phase_and_process_tree_resources_verified"
    )
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["proof_digest"] is not None


def test_verdict_host_profile_fingerprint_is_enforced(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 2)
    monkeypatch.setattr(pairrun, "FRESH_ROOTS_FLOOR", 1)
    st = _pair_stage(tmp_path, scope="qualified", claims={"speed": True})
    profile_path = st["stage"] / "host-profile.json"
    profile = json.loads(profile_path.read_text(encoding="utf-8"))
    profile["fingerprint"]["cpu_count"] += 1
    profile_path.write_text(json.dumps(profile), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["provenance"]["host"].update(
            profile_digest=pairrun.sha_file(profile_path)
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"].startswith("admission_unverified:")
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"] == "protocol_lock_host_profile_drift"
    assert verdict["failure_class"] == "provenance"


def test_verdict_rejects_forged_phase_and_process_tree_resources(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 2)
    monkeypatch.setattr(pairrun, "FRESH_ROOTS_FLOOR", 1)

    resource_stage = _pair_stage(tmp_path / "resource", scope="qualified", claims={"speed": True})
    resource_path = resource_stage["stage"] / "rep-00" / "semble-resource-metrics.json"
    resource = json.loads(resource_path.read_text(encoding="utf-8"))
    resource.update(complete=False, error="sampler lost process tree")
    resource_path.write_text(json.dumps(resource), encoding="utf-8")
    verdict = _stage_verdict(resource_stage)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "resource_accounting_incomplete"
    )

    phase_stage = _pair_stage(tmp_path / "phase", scope="qualified", claims={"speed": True})
    phase_path = (
        phase_stage["stage"] / "rep-00" / "quanta" / "strategy-00-whole_file" / "phase-metrics.json"
    )
    phase = json.loads(phase_path.read_text(encoding="utf-8"))
    phase["total_ms"] += 1.0
    phase_path.write_text(json.dumps(phase), encoding="utf-8")
    verdict = _stage_verdict(phase_stage)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == ("phase_boundaries_incomplete")
    schedule_stage = _pair_stage(tmp_path / "schedule", scope="qualified", claims={"speed": True})
    schedule_path = schedule_stage["stage"] / "rep-00" / "semble" / "phase-metrics.json"
    schedule = json.loads(schedule_path.read_text(encoding="utf-8"))
    schedule["query_schedule"].reverse()
    schedule_path.write_text(json.dumps(schedule), encoding="utf-8")
    verdict = _stage_verdict(schedule_stage)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == ("phase_boundaries_incomplete")
    memory_stage = _pair_stage(tmp_path / "memory", scope="qualified", claims={"speed": True})
    native_path = memory_stage["stage"] / "rep-00" / "semble" / "native.json"
    native = json.loads(native_path.read_text(encoding="utf-8"))
    native["stats"]["index_resident_bytes"] += 1
    native_path.write_text(json.dumps(native), encoding="utf-8")
    verdict = _stage_verdict(memory_stage)
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "resource_accounting_incomplete"
    )
    # T12: a speed claim without a declared cache regime fails once every
    # earlier gate holds; the declared stage reaches the phase frontier.
    with pytest.raises(pairrun.RunError, match="qualified admission cannot use"):
        _pair_stage(
            tmp_path / "cache", scope="qualified", claims={"speed": True}, cache_regime="undeclared"
        )


def test_verdict_t15_t16_conditionals(tmp_path):
    st = _pair_stage(tmp_path, claims={"same_model": True})
    verdict = _stage_verdict(st)
    assert "T15" in verdict["missing_t_ids"]
    assert verdict["failure_class"] == "model"
    parity = {"model_parity_results": _parity_results("parity-cmd")}
    st = _pair_stage(tmp_path / "parity", claims={"same_model": True}, receipts=parity)
    verdict = _stage_verdict(st)
    assert "T15" not in verdict["missing_t_ids"]
    assert verdict["failure_class"] == "none"
    bad = {"incremental_results": _parity_results("incr-cmd", status="fail", failed=4)}
    st = _pair_stage(tmp_path / "incr", claims={"incremental": True}, receipts=bad)
    verdict = _stage_verdict(st)
    assert "T16" in verdict["missing_t_ids"]
    assert verdict["failure_class"] == "infra"


def test_verdict_quality_gates(tmp_path, monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    st = _pair_stage(tmp_path, claims={"quality": True})
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "not_applicable"
    assert "scope:exploratory_only" in verdict["not_applicable_t_ids"]
    st = _pair_stage(
        tmp_path / "iso", blinding="isolated", scope="qualified", claims={"quality": True}
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "pass"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == ("blinded_graded_delta")
    assert verdict["failure_class"] == "none"
    proof_path = st["stage"] / "isolation-proof.json"
    proof = json.loads(proof_path.read_text(encoding="utf-8"))
    proof["profile_sha256"] = _fake_sha("forged-profile")
    proof_path.write_text(json.dumps(proof), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"].startswith(
        "isolation_proof_unverified:"
    )
    assert verdict["failure_class"] == "blinding"
    st = _pair_stage(
        tmp_path / "ungraded",
        blinding="isolated",
        scope="qualified",
        claims={"quality": True},
        graded=False,
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == "reports_ungraded"
    assert verdict["failure_class"] == "scoring"
    # A manifest cannot upgrade attested records to isolated quality proof.
    st = _pair_stage(tmp_path / "spoof", scope="qualified", claims={"quality": True})
    _rewrite_manifest(st, lambda m: m.update(blinding="isolated"))
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == ("isolation_record_mismatch")
    assert verdict["blinding"] == "attested"
    # T10: a quality claim over the hash-dev diagnostic control fails even
    # when every other quality gate would pass.
    st = _pair_stage(
        tmp_path / "hashdev",
        blinding="isolated",
        scope="qualified",
        claims={"quality": True},
        embedder="hash-dev",
    )
    assert st["manifest"]["provenance"]["quanta"]["embedder"] == "hash-dev"
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == "model_quality_embedder"
    assert verdict["failure_class"] == "model"
    assert verdict["provenance"]["quanta"]["embedder"] == "hash-dev"


def test_qualified_admission_is_reverified_after_capture(tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    annotation = st["stage"] / "admission" / "annotation-1-receipt.json"
    annotation.write_text('{"annotator":"post-capture-tamper"}', encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"].startswith("admission_unverified:")
    assert verdict["failure_class"] == "admission"


def test_shared_query_protocol_is_deterministic_digest_bound_and_permuted():
    task_ids = [f"T{index:02d}" for index in range(20)]
    first = pairrun.build_query_protocol(task_ids, 17, 1, 10)
    second = pairrun.build_query_protocol(task_ids, 17, 1, 10)
    assert first == second
    assert pairrun.validate_query_protocol(first, task_ids, "protocol") == first
    assert all(sorted(schedule) == task_ids for schedule in first["measurement_schedules"])

    mutated = json.loads(json.dumps(first))
    mutated["measurement_schedules"][0].reverse()
    with pytest.raises(pairrun.RunError, match="sha256 mismatch"):
        pairrun.validate_query_protocol(mutated, task_ids, "protocol")

    self_consistent_forgery = json.loads(json.dumps(first))
    self_consistent_forgery["measurement_schedules"][0].reverse()
    core = {key: value for key, value in self_consistent_forgery.items() if key != "sha256"}
    self_consistent_forgery["sha256"] = pairrun._protocol_digest(core)
    with pytest.raises(pairrun.RunError, match="deterministic seeded schedule"):
        pairrun.validate_query_protocol(self_consistent_forgery, task_ids, "protocol")

    malformed = json.loads(json.dumps(first))
    malformed["measurement_schedules"][0][0] = 7
    with pytest.raises(pairrun.RunError, match="exact task permutation"):
        pairrun.validate_query_protocol(malformed, task_ids, "protocol")


@pytest.mark.parametrize(
    ("spec", "task_count", "message"),
    [
        (
            {
                "repetitions": 4,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": 13,
                "routes": ["hybrid"],
            },
            20,
            "fresh roots",
        ),
        (
            {
                "repetitions": 5,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": 10,
                "routes": ["hybrid"],
            },
            19,
            "20 frozen tasks",
        ),
        (
            {
                "repetitions": 5,
                "query_warmup_passes": 0,
                "query_repetitions_per_root": 10,
                "routes": ["hybrid"],
            },
            20,
            "warmup pass",
        ),
        (
            {
                "repetitions": 5,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": 9,
                "routes": ["hybrid"],
            },
            20,
            "warm observations",
        ),
        (
            {
                "repetitions": 5,
                "query_warmup_passes": 1,
                "query_repetitions_per_root": 10,
                "routes": ["lexical", "hybrid"],
            },
            20,
            "exactly one Quanta route",
        ),
    ],
)
def test_qualified_speed_spec_rejects_underpowered_or_biased_protocol(spec, task_count, message):
    with pytest.raises(pairrun.RunError, match=message):
        pairrun.validate_qualified_speed_spec(spec, task_count)


def test_qualified_speed_spec_accepts_1000_warm_observations_per_route():
    pairrun.validate_qualified_speed_spec(
        {
            "repetitions": 5,
            "query_warmup_passes": 1,
            "query_repetitions_per_root": 10,
            "routes": ["hybrid"],
        },
        20,
    )


def test_protocol_phase_metrics_bind_raw_warm_counts_and_cold_separately():
    task_ids = ["T1", "T2"]
    protocol = pairrun.build_query_protocol(task_ids, 3, 1, 2)
    phase = {
        "schema_version": 1,
        "system": "quanta",
        "timing_layer": "runner_monotonic_wall_v1",
        "strategy": "whole_file",
        "record_sha256": "a" * 64,
        "runner_binary_sha256": "b" * 64,
        "task_count": 2,
        "route_count": 1,
        "file_count": 1,
        "chunk_count": 1,
        "query_schedule": task_ids,
        "warmup_passes": 1,
        "measurement_repetitions": 2,
        "query_protocol": protocol,
        "warm_latencies_ms": {"hybrid": {"T1": [1.0, 1.1], "T2": [2.0, 2.1]}},
        "cold_latencies_ms": {"hybrid": 3.0},
        "phases_ms": {
            "discovery": 1.0,
            "chunk": 1.0,
            "model_provider_prepare": 1.0,
            "embed_publish_seal_activate": 1.0,
            "cold_query": 3.0,
            "warmup": 1.0,
            "warm_query": 7.0,
            "unattributed": 1.0,
        },
        "total_ms": 16.0,
    }
    assert pairrun._validate_phase_metrics(phase, "phase") == phase
    phase["warm_latencies_ms"]["hybrid"]["T1"].pop()
    with pytest.raises(pairrun.RunError, match="count differs"):
        pairrun._validate_phase_metrics(phase, "phase")


def test_protocol_phase_metrics_reject_samples_longer_than_enclosing_phases(tmp_path):
    st = _pair_stage(tmp_path)
    qphase = json.loads(
        Path(st["rep_layouts"][0]["quanta_phase_metrics"]["whole_file"]).read_text()
    )
    qphase["cold_latencies_ms"]["lexical"] = 1e12
    with pytest.raises(pairrun.RunError, match="cold samples exceed"):
        pairrun._validate_phase_metrics(qphase, "phase")
    qphase["cold_latencies_ms"]["lexical"] = 1.0
    qphase["warm_latencies_ms"]["lexical"][qphase["query_schedule"][0]][0] = 1e12
    with pytest.raises(pairrun.RunError, match="warm samples exceed"):
        pairrun._validate_phase_metrics(qphase, "phase")


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("top_k", 999),
        ("repetitions", 999),
        ("strategies", ["not-executed"]),
        ("searchd_expected_sha256", "0" * 64),
        ("semble_lockfile_sha256", "0" * 64),
        ("host_profile_digest", "0" * 64),
        ("unknown_authority", True),
    ],
)
def test_verdict_rejects_protocol_lock_pin_mutations(tmp_path, field, value):
    st = _pair_stage(tmp_path)
    baseline = _stage_verdict(st)
    assert baseline["states"]["PAIR_VALID"] == "pass"
    lock = st["stage"] / "protocol-lock.json"
    payload = json.loads(lock.read_text())
    payload[field] = value
    lock.write_text(json.dumps(payload), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert "protocol_lock" in verdict["state_evidence"]["PAIR_VALID"]["reason"]


def test_qualified_replay_rejects_two_task_protocol_even_with_1000_samples(tmp_path):
    st = _pair_stage(tmp_path, repetitions=5, scope="qualified", claims={"speed": True})
    protocol_digests = []
    for layout in st["rep_layouts"]:
        protocol_path = Path(layout["query_protocol"])
        previous = json.loads(protocol_path.read_text())
        protocol = pairrun.build_query_protocol(previous["task_ids"], previous["seed"], 1, 100)
        protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
        protocol_digests.append(protocol["sha256"])
        for phase_path in (
            Path(layout["quanta_phase_metrics"]["whole_file"]),
            Path(layout["semble_phase_metrics"]),
        ):
            phase = json.loads(phase_path.read_text())
            phase["query_protocol"] = protocol
            phase["measurement_repetitions"] = 100
            for by_task in phase["warm_latencies_ms"].values():
                for task_id, values in by_task.items():
                    by_task[task_id] = values * 100
            phase["phases_ms"]["warm_query"] += 297.0
            phase["total_ms"] += 297.0
            if phase["system"] == "semble":
                phase["phase_boundaries_ns"]["query_end"] += 297_000_000
                phase["phase_boundaries_ns"]["worker_end"] += 297_000_000
            phase_path.write_text(json.dumps(phase), encoding="utf-8")
        quanta_manifest_path = Path(layout["quanta_manifest"])
        quanta_manifest = json.loads(quanta_manifest_path.read_text())
        quanta_manifest["runs"][0]["phase_metrics_digest"] = pairrun.sha_file(
            Path(layout["quanta_phase_metrics"]["whole_file"])
        )
        quanta_manifest_path.write_text(json.dumps(quanta_manifest), encoding="utf-8")
    lock_path = st["stage"] / "protocol-lock.json"
    lock = json.loads(lock_path.read_text())
    lock["query_repetitions_per_root"] = 100
    lock["query_protocol_sha256s"] = protocol_digests
    lock_path.write_text(json.dumps(lock), encoding="utf-8")
    matrix = pairrun.build_latency_matrix(st["rep_layouts"])
    assert matrix["observations_floor"] == 1000
    (st["stage"] / "latency-matrix.json").write_text(json.dumps(matrix), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["evidence"]["perf"].update(
            observations_floor=matrix["observations_floor"], fresh_roots=matrix["fresh_roots"]
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "pass"
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert "at least 20 frozen tasks" in verdict["state_evidence"]["PERF_QUALIFIED"]["reason"]


def test_verdict_cannot_qualify_cold_only_latency_as_warm_performance(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 2)
    monkeypatch.setattr(pairrun, "FRESH_ROOTS_FLOOR", 1)
    st = _pair_stage(tmp_path, scope="qualified", claims={"speed": True})
    layout = st["rep_layouts"][0]
    qphase_path = Path(layout["quanta_phase_metrics"]["whole_file"])
    qphase = json.loads(qphase_path.read_text(encoding="utf-8"))
    for key in ("query_protocol", "warm_latencies_ms", "cold_latencies_ms"):
        qphase.pop(key)
    qphase["phases_ms"]["first_query"] = qphase["phases_ms"].pop("cold_query")
    qphase["phases_ms"].pop("warmup")
    qphase["warmup_passes"] = 0
    qphase["total_ms"] = 7.0
    qphase_path.write_text(json.dumps(qphase), encoding="utf-8")
    quanta_manifest_path = st["stage"] / "rep-00" / "quanta" / "quanta-manifest.json"
    quanta_manifest = json.loads(quanta_manifest_path.read_text(encoding="utf-8"))
    quanta_manifest["runs"][0]["phase_metrics_digest"] = ev.digest(qphase_path.read_bytes())
    quanta_manifest_path.write_text(json.dumps(quanta_manifest), encoding="utf-8")

    sphase_path = Path(layout["semble_phase_metrics"])
    sphase = json.loads(sphase_path.read_text(encoding="utf-8"))
    for key in ("query_protocol", "warm_latencies_ms", "cold_latencies_ms"):
        sphase.pop(key)
    sphase["phases_ms"]["first_query"] = sphase["phases_ms"].pop("cold_query")
    sphase["phases_ms"]["warm_query"] = 1.0
    sphase["phase_boundaries_ns"] = {
        "worker_start": 0,
        "discovery_end": 1_000_000,
        "model_provider_prepare_end": 2_000_000,
        "index_end": 3_000_000,
        "warmup_end": 4_000_000,
        "query_start": 4_000_000,
        "first_query_start": 4_000_000,
        "first_query_end": 5_000_000,
        "query_end": 6_000_000,
        "worker_end": 7_000_000,
    }
    sphase["total_ms"] = 7.0
    sphase_path.write_text(json.dumps(sphase), encoding="utf-8")

    legacy_layout = dict(layout)
    legacy_layout.pop("query_protocol")
    legacy_layout.pop("quanta_phase_metrics")
    matrix = pairrun.build_latency_matrix([legacy_layout])
    (st["stage"] / "latency-matrix.json").write_text(json.dumps(matrix), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["evidence"]["perf"].update(
            observations_floor=matrix["observations_floor"],
            fresh_roots=matrix["fresh_roots"],
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == "phase_boundaries_incomplete"


def test_verdict_rejects_nonconsecutive_or_unbound_root_protocols(tmp_path, monkeypatch):
    monkeypatch.setattr(pairrun, "PILOT_OBSERVATIONS_FLOOR", 2)
    monkeypatch.setattr(pairrun, "FRESH_ROOTS_FLOOR", 1)
    st = _pair_stage(tmp_path, repetitions=2, scope="qualified", claims={"speed": True})
    protocol_path = st["stage"] / "protocol-lock.json"
    protocol = json.loads(protocol_path.read_text(encoding="utf-8"))
    protocol["base_seed"] += 1
    protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == (
        "query_protocol_root_sequence_unverified"
    )


def test_source_closure_driver_uses_canonical_capture_and_verify_commands(tmp_path, monkeypatch):
    observed = []

    def fake_run(command, **kwargs):
        observed.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, stdout="ok", stderr="")

    monkeypatch.setattr(pairrun.subprocess, "run", fake_run)
    closure = tmp_path / "closure.json"
    pairrun._source_closure(tmp_path, "capture", closure)
    pairrun._source_closure(tmp_path, "verify", closure)
    assert observed[0][0][-5:] == [
        "capture",
        "--profile",
        "retrieval",
        "--out",
        str(closure),
    ]
    assert observed[1][0][-3:] == ["verify", "--manifest", str(closure)]
    assert all(call[1]["cwd"] == tmp_path and call[1]["timeout"] == 300 for call in observed)


def test_retrieval_source_closure_profile_covers_authority_surfaces():
    profile = source_closure.PROFILES["retrieval"]
    paths = set(profile["paths"])
    assert {
        "Cargo.lock",
        "Cargo.toml",
        "Justfile",
        "docs/plans/sep-23-retrieval-bench",
        "tools/benchmark/retrieval",
        "tools/ci/nextest_events.py",
        "tools/ci/source_closure.py",
        "tools/ci/tests/test_retrieval_benchmark.py",
        "tools/ci/tests/test_retrieval_contract_proof.py",
        "tools/ci/tests/test_retrieval_sdk_proof.py",
        "tools/ci/write-verification-receipt.py",
    } <= paths
    assert set(profile["cargo_packages"]) == {
        "quanta-index-retrieval-bench",
        "quanta-index-searchd-runtime",
    }


def test_source_closure_driver_fails_closed_on_tool_refusal(tmp_path, monkeypatch):
    monkeypatch.setattr(
        pairrun.subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess(args[0], 2, stdout="", stderr="dirty"),
    )
    with pytest.raises(pairrun.RunError, match="source-closure capture refused: dirty"):
        pairrun._source_closure(tmp_path, "capture", tmp_path / "closure.json")
    with pytest.raises(pairrun.RunError, match="unsupported source-closure command"):
        pairrun._source_closure(tmp_path, "check", None)


def test_verdict_cannot_upgrade_qualified_warm_cache(tmp_path):
    st = _pair_stage(tmp_path, scope="qualified", claims={"speed": True}, cache_regime="warm_cache")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PERF_QUALIFIED"] == "fail"
    assert verdict["state_evidence"]["PERF_QUALIFIED"]["reason"] == ("unsupported_cache_protocol")


def test_qualified_quality_requires_estimable_uncertainty(tmp_path):
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert verdict["state_evidence"]["QUALITY_DELTA"]["reason"] == "uncertainty_unqualified"


def test_qualified_verdict_refuses_tampered_driver_source_closure(tmp_path):
    st = _pair_stage(tmp_path, scope="qualified")
    closure_path = st["stage"] / "driver-source-closure.json"
    closure = json.loads(closure_path.read_text(encoding="utf-8"))
    closure["files"][0]["sha256"] = _fake_sha("tampered-source")
    closure_path.write_text(json.dumps(closure), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="driver source closure.digest mismatch"):
        _stage_verdict(st)


def test_qualified_verdict_refuses_valid_but_unbound_driver_source_closure(tmp_path):
    st = _pair_stage(tmp_path, scope="qualified")
    closure_path = st["stage"] / "driver-source-closure.json"
    closure = json.loads(closure_path.read_text(encoding="utf-8"))
    closure["files"][0]["sha256"] = _fake_sha("different-source")
    core = {
        key: closure[key] for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    closure["digest"] = ev.digest(ev.canonical(core))
    closure_path.write_text(json.dumps(closure), encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["PAIR_VALID"] == "fail"
    assert verdict["state_evidence"]["PAIR_VALID"]["reason"] == (
        "driver_source_closure_digest_drift"
    )


def test_qualified_verdict_refuses_receipt_capture_closure_mismatch(tmp_path, monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    receipt_path = st["stage"] / "receipts" / "sdk_receipt.json"
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    receipt["source_closure"]["files"][0]["sha256"] = _fake_sha("other-source")
    core = {
        key: receipt["source_closure"][key]
        for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    receipt["source_closure"]["digest"] = ev.digest(ev.canonical(core))
    receipt_path.write_text(json.dumps(receipt), encoding="utf-8")

    admission_path = st["stage"] / "admission" / "admission.json"
    admission = json.loads(admission_path.read_text(encoding="utf-8"))
    admission["verification"]["sdk_receipt_sha256"] = pairrun.sha_file(receipt_path)
    admission_path.write_text(json.dumps(admission), encoding="utf-8")
    admission_digest = pairrun.sha_file(admission_path)
    protocol_path = st["stage"] / "protocol-lock.json"
    protocol = json.loads(protocol_path.read_text(encoding="utf-8"))
    protocol["admission_digest"] = admission_digest
    protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
    _rewrite_manifest(
        st,
        lambda manifest: manifest["provenance"]["admission"].update(
            manifest_digest=admission_digest
        ),
    )
    verdict = _stage_verdict(st)
    assert verdict["states"]["SDK_PATH_GREEN"] == "fail"
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert (
        "source closure differs from capture closure"
        in verdict["state_evidence"]["QUALITY_DELTA"]["reason"]
    )


def test_isolation_proof_refuses_tampered_frozen_runner_tool(tmp_path, monkeypatch):
    monkeypatch.setattr(ev, "MIN_CI_SAMPLE", 2)
    st = _pair_stage(tmp_path, blinding="isolated", scope="qualified", claims={"quality": True})
    (st["stage"] / "runner-tools" / "semble.py").write_text("tampered adapter", encoding="utf-8")
    verdict = _stage_verdict(st)
    assert verdict["states"]["QUALITY_DELTA"] == "fail"
    assert "runner tool digest mismatch" in verdict["state_evidence"]["QUALITY_DELTA"]["reason"]


def test_runtime_manifest_requires_qualified_source_closure_artifact(tmp_path):
    st = _pair_stage(tmp_path, scope="qualified")
    _rewrite_manifest(st, lambda manifest: manifest["artifacts"].pop("driver_source_closure"))
    with pytest.raises(pairrun.RunError, match="driver source closure"):
        _stage_verdict(st)


def test_verdict_refusals(tmp_path):
    st = _pair_stage(tmp_path)
    (st["stage"] / "rep-00" / "semble" / "native.json").unlink()
    with pytest.raises(pairrun.RunError, match="artifact is missing"):
        _stage_verdict(st)
    st = _pair_stage(tmp_path / "suite")
    foreign = st["stage"] / "foreign-suite.json"
    drifted = json.loads(json.dumps(st["suite"]))
    drifted["suite_id"] = "foreign-suite"
    foreign.write_text(json.dumps(drifted), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="CLI suite differs"):
        pairrun.build_verdict(st["repo"], foreign, st["manifest_path"])
    st = _pair_stage(tmp_path / "shape")
    st["manifest_path"].write_text('{"manifest_version": 1}', encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="must hold exactly"):
        _stage_verdict(st)


def test_verdict_cli_smoke(tmp_path):
    st = _pair_stage(tmp_path)
    out = st["stage"] / "cli-verdict.json"
    completed = subprocess.run(
        [
            sys.executable,
            str(Path(pairrun.__file__)),
            "verdict",
            "--repo",
            str(st["repo"]),
            "--suite",
            str(st["suite_path"]),
            "--run-manifest",
            str(st["manifest_path"]),
            "--out",
            str(out),
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert completed.returncode == 0, completed.stderr
    assert json.loads(out.read_text(encoding="utf-8"))["states"] == _stage_verdict(st)["states"]


def test_successful_promotion_replays_identically_in_new_process(tmp_path):
    st = _pair_stage(tmp_path)
    before = _stage_verdict(st)
    assert before["states"]["PAIR_VALID"] == "pass"
    old_stage = st["stage"]
    promoted = old_stage.with_name("promoted")
    old_stage.rename(promoted)
    output = tmp_path / "promoted-verdict.json"
    completed = subprocess.run(
        [
            sys.executable,
            str(Path(pairrun.__file__)),
            "verdict",
            "--repo",
            str(st["repo"]),
            "--suite",
            str(promoted / st["suite_path"].relative_to(old_stage)),
            "--run-manifest",
            str(promoted / "run-manifest.json"),
            "--out",
            str(output),
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert completed.returncode == 0, completed.stderr
    after = json.loads(output.read_text(encoding="utf-8"))
    assert after["states"] == before["states"]
    assert after["state_evidence"]["PAIR_VALID"] == before["state_evidence"]["PAIR_VALID"]


def test_run_pair_promotes_complete_stage_and_public_verdict_replays(tmp_path, monkeypatch, capsys):
    st = _pair_stage(tmp_path / "fixture")
    original = _stage_verdict(st)
    output_root = tmp_path / "published"
    monkeypatch.setattr(pairrun, "preflight_capture", lambda _spec: output_root)

    def staged(_spec, stage):
        shutil.copytree(st["stage"], stage, dirs_exist_ok=True)
        return {"status": "staged"}

    monkeypatch.setattr(pairrun, "_run_pair_staged", staged)
    assert pairrun.run_pair(
        {
            "scope": "exploratory",
            "semble_lockfile_sha256": _fake_sha("lock"),
            "semble_python": "python3",
            "semble_lockfile": "lockfile",
            "host_profile": "host-profile",
        }
    ) == 0
    assert json.loads(capsys.readouterr().out)["output_root"] == str(output_root)
    assert not output_root.with_name(output_root.name + ".staging").exists()
    public = subprocess.run(
        [
            sys.executable,
            str(Path(pairrun.__file__)),
            "verdict",
            "--repo",
            str(st["repo"]),
            "--suite",
            str(output_root / st["suite_path"].relative_to(st["stage"])),
            "--run-manifest",
            str(output_root / "run-manifest.json"),
            "--out",
            str(tmp_path / "replayed.json"),
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert public.returncode == 0, public.stderr
    replayed = json.loads((tmp_path / "replayed.json").read_text())
    assert replayed["states"] == original["states"]
    assert replayed["state_evidence"]["PAIR_VALID"] == original["state_evidence"]["PAIR_VALID"]


def test_pair_staging_atomicity(tmp_path, monkeypatch):
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path / "src")
    manifest = tmp_path / "manifest.json"
    manifest.write_text(
        json.dumps({"repository_commit": suite["repository_commit"]}), encoding="utf-8"
    )
    suite_file = tmp_path / "suite.json"
    suite_file.write_text(json.dumps(suite), encoding="utf-8")
    _suite, pack, _source = ev.validate_suite(repo, suite)
    pack_file = tmp_path / "pack.json"
    pack_file.write_text(json.dumps(pack), encoding="utf-8")
    searchd = tmp_path / "searchd"
    searchd.write_bytes(b"searchd")
    lockfile = tmp_path / "semble-lock.txt"
    lockfile.write_bytes(b"semble==0.6.0\n")
    host_profile = tmp_path / "host-profile.json"
    host_profile.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "profile_id": "test-host",
                "fingerprint": {
                    "system": "Darwin",
                    "release": "test",
                    "machine": "arm64",
                    "processor": "test",
                    "cpu_count": 8,
                    "rustc": "rustc test",
                    "power_digest": _fake_sha("power"),
                },
            }
        ),
        encoding="utf-8",
    )
    spec = {
        "repo": str(repo),
        "manifest": str(manifest),
        "suite": str(suite_file),
        "query_pack": str(pack_file),
        "top_k": 10,
        "output_root": str(tmp_path / "out"),
        "runner_binary": "/unused/runner",
        "strategies": [{"name": "whole_file"}],
        "searchd_binary": str(searchd),
        "searchd_expected_sha256": ev.digest(b"searchd"),
        "semble_python": "/unused/python",
        "semble_lockfile": str(lockfile),
        "semble_lockfile_sha256": _fake_sha("lock"),
        "host_profile": str(host_profile),
    }

    def explode(_spec, _spec_dir):
        raise pairrun.RunError("boom")

    monkeypatch.setattr(pairrun, "run_quanta", explode)
    with pytest.raises(pairrun.RunError, match="boom"):
        pairrun.run_pair(spec)
    assert not (tmp_path / "out").exists()
    stage = tmp_path / "out.staging"
    assert stage.is_dir()
    assert list(stage.rglob("verdict.json")) == []


def test_matrix_floor_no_cross_strategy_inflation():
    cells = [
        {
            "system": "quanta",
            "strategy": "whole_file",
            "rows": [("lexical", f"T{i}", "success", 1.0) for i in range(100)],
            "native_latencies": None,
            "native_route": None,
        },
        {
            "system": "quanta",
            "strategy": "brace_heuristic",
            "rows": [("lexical", f"T{i}", "success", 1.0) for i in range(3)],
            "native_latencies": None,
            "native_route": None,
        },
    ]
    matrix = pairrun.aggregate_matrix(cells, 1)
    assert matrix["floors"] == {
        "quanta:brace_heuristic:lexical": 3,
        "quanta:whole_file:lexical": 100,
    }
    assert matrix["observations_floor"] == 3
    cells = [
        {
            "system": "semble",
            "strategy": "native",
            "rows": [
                ("hybrid", "T1", "success", 2.0),
                ("hybrid", "T2", "abstained", 1.0),
                ("hybrid", "T3", "timeout", 5.0),
                ("hybrid", "T4", "success", None),
            ],
            "native_latencies": {"T1": [2.0, 2.5], "T4": [0.5]},
            "native_route": "hybrid",
        }
    ]
    matrix = pairrun.aggregate_matrix(cells, 1)
    key = "semble:native:hybrid"
    assert matrix["attempts"][key] == 4
    assert matrix["errors"][key] == 1
    assert matrix["nulls"][key] == 1
    assert matrix["abstained"][key] == 1
    assert matrix["floors"][key] == 4
    bad = json.loads(json.dumps(cells))
    bad[0]["native_latencies"]["T1"] = [9.9, 2.5]
    with pytest.raises(pairrun.RunError, match="disagree"):
        pairrun.aggregate_matrix(bad, 1)


def test_matrix_uses_only_shared_warm_samples_and_rejects_first_sample_drift():
    cell = {
        "system": "quanta",
        "strategy": "whole_file",
        "rows": [("hybrid", "T1", "success", 2.0)],
        "warm_latencies": {"hybrid": {"T1": [2.0, 2.5, 3.0]}},
        "cold_latencies": {"hybrid": 99.0},
        "native_latencies": None,
        "native_route": None,
    }
    matrix = pairrun.aggregate_matrix([cell], 1)
    assert matrix["samples"]["quanta:whole_file:hybrid:T1"] == [2.0, 2.5, 3.0]
    assert matrix["observations_floor"] == 3
    assert 99.0 not in matrix["samples"]["quanta:whole_file:hybrid:T1"]

    cell["warm_latencies"]["hybrid"]["T1"][0] = 2.1
    with pytest.raises(pairrun.RunError, match="disagree"):
        pairrun.aggregate_matrix([cell], 1)


def test_receipt_shape_mirrors_canonical_schema():
    canonical = json.loads(
        Path("tools/ci/verification-receipt.schema.json").read_text(encoding="utf-8")
    )
    results = _counts_results("cmd", 2, 2, 2, 0)
    receipt = _receipt(
        "cmd",
        json.dumps(results).encode(),
        "abcdef123456" + "0" * 28,
        "probe",
        {"raw": b"raw evidence"},
    )
    jsonschema.validate(receipt, canonical)
    assert pairrun._validate_receipt_shape(receipt, "probe") == receipt
    for key in ("tier", "test_event_count", "revision"):
        mutant = dict(receipt)
        mutant[key] = "bogus" if key != "test_event_count" else 0
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(mutant, canonical)
        with pytest.raises(pairrun.RunError):
            pairrun._validate_receipt_shape(mutant, "probe")


def test_host_probe_records_without_fabrication():
    probe = pairrun.host_probe()
    for key in (
        "system",
        "machine",
        "cpu_count",
        "python",
        "concurrent_processes",
        "thermal",
        "frequency",
        "power",
    ):
        assert key in probe, f"host probe lacks {key}"


def test_darwin_power_fingerprint_excludes_battery_observation(monkeypatch):
    monkeypatch.setattr(pairrun.sys, "platform", "darwin")
    settings = """Battery Power:
 lidwake              1
 lowpowermode         1
AC Power:
 lidwake              1
 lowpowermode         0
"""
    source = [
        "Now drawing from 'AC Power'\n -InternalBattery-0 80%; charging; 1:00 remaining",
        "Now drawing from 'AC Power'\n -InternalBattery-0 81%; charging; 0:55 remaining",
    ]

    def fake_run(command, **_kwargs):
        if command[-1] == "custom":
            return subprocess.CompletedProcess(command, 0, settings, "")
        return subprocess.CompletedProcess(command, 0, source.pop(0), "")

    monkeypatch.setattr(pairrun.subprocess, "run", fake_run)
    first = pairrun.read_power()
    second = pairrun.read_power()
    assert first["status"] == second["status"] == "bounded"
    assert first["digest"] == second["digest"]
    assert first["observation_digest"] != second["observation_digest"]
    assert first["active_source"] == "AC Power"


def test_darwin_thermal_limits_and_frequency_fail_closed(monkeypatch):
    monkeypatch.setattr(pairrun.sys, "platform", "darwin")
    monkeypatch.setattr(pairrun.os, "cpu_count", lambda: 8)

    def throttled(command, **_kwargs):
        return subprocess.CompletedProcess(
            command,
            0,
            "CPU_Scheduler_Limit = 100\nCPU_Available_CPUs = 8\nCPU_Speed_Limit = 50\n",
            "",
        )

    monkeypatch.setattr(pairrun.subprocess, "run", throttled)
    assert pairrun.read_thermal()["status"] == "warning"
    monkeypatch.setattr(
        pairrun,
        "read_sysctl",
        lambda _keys: {
            "hw.cpufrequency": "unavailable",
            "hw.cpufrequency_max": "unavailable",
        },
    )
    assert pairrun.read_frequency({"status": "bounded"})["status"] == "unavailable"

    monkeypatch.setattr(
        pairrun,
        "read_sysctl",
        lambda _keys: {"hw.cpufrequency": "2400000000", "hw.cpufrequency_max": "3200000000"},
    )
    assert pairrun.read_frequency()["status"] == "bounded"


@pytest.mark.skipif(
    not pairrun.SANDBOX_EXEC.is_file(),
    reason="macOS Seatbelt backend is unavailable",
)
def test_seatbelt_profile_is_default_deny_allowlist(tmp_path):
    allowed = tmp_path / "allowed.json"
    denied = tmp_path / "unrelated-secret.json"
    allowed.write_text("allowed", encoding="utf-8")
    denied.write_text("secret", encoding="utf-8")
    profile = pairrun._seatbelt_profile([], [str(allowed)], [])
    assert "(deny default)" in profile
    assert "(allow default)" not in profile
    allowed_probe = subprocess.run(
        [str(pairrun.SANDBOX_EXEC), "-p", profile, "/bin/cat", str(allowed)],
        capture_output=True,
        text=True,
    )
    denied_probe = subprocess.run(
        [str(pairrun.SANDBOX_EXEC), "-p", profile, "/bin/cat", str(denied)],
        capture_output=True,
        text=True,
    )
    assert allowed_probe.returncode == 0 and allowed_probe.stdout == "allowed"
    assert denied_probe.returncode != 0 and denied_probe.stdout == ""


# --- G0: W0-A comparison-contract cutover (schema v3) ---

SCHEMA_DIR = Path(ev.__file__).resolve().parent
G0_SCHEMAS = (
    "runner.schema.json",
    "suite.schema.json",
    "pair-spec.schema.json",
    "admission.schema.json",
    "run-manifest.schema.json",
    "verdict.schema.json",
)


def _load_schema(name: str) -> dict:
    return json.loads((SCHEMA_DIR / name).read_text(encoding="utf-8"))


def _fake_sha(seed: str) -> str:
    return ev.digest(f"g0-seed-{seed}".encode())


def test_g0_schema_files_are_closed():
    """Every object node constrains its values: false or a value schema, never open."""
    for name in G0_SCHEMAS:
        schema = _load_schema(name)
        open_nodes = []

        def walk(node, path="$", open_nodes=open_nodes):
            if isinstance(node, dict):
                if node.get("type") == "object":
                    guard = node.get("additionalProperties")
                    if guard is not False and not isinstance(guard, dict):
                        open_nodes.append(path)
                for key, value in node.items():
                    walk(value, f"{path}.{key}")
            elif isinstance(node, list):
                for index, value in enumerate(node):
                    walk(value, f"{path}[{index}]")

        walk(schema)
        assert not open_nodes, f"{name} has unconstrained objects: {open_nodes}"


def test_g0_receipt_shape_matches_canonical_receipt_schema():
    canonical = json.loads(
        Path("tools/ci/verification-receipt.schema.json").read_text(encoding="utf-8")
    )
    embedded = _load_schema("run-manifest.schema.json")["$defs"]["verification_receipt"]
    for key in ("type", "additionalProperties", "required", "properties", "oneOf"):
        assert embedded[key] == canonical[key], f"receipt $def drifted on {key}"


def test_g0_pair_spec_receipts_are_paths_not_content():
    pair_full = _load_schema("pair-spec.schema.json")
    manifest_full = _load_schema("run-manifest.schema.json")
    pair_receipts = pair_full["properties"]["receipts"]
    manifest_artifacts = manifest_full["properties"]["artifacts"]
    assert sorted(pair_receipts["properties"]) == sorted(pairrun.RECEIPT_KEYS)
    # Every spec receipt path lands on a manifest artifact of the same name.
    for key in pairrun.RECEIPT_KEYS:
        assert key in manifest_artifacts["properties"], key
        assert pair_receipts["properties"][key] == {"type": "string", "minLength": 1}
    # Evidence content is rejected in the spec: paths only.
    spec = _g0_spec()
    spec["receipts"] = {"contract_python_results": "/tmp/results.json"}
    jsonschema.validate(spec, pair_full)
    spec["receipts"] = {"contract_python_results": {"passed": 10}}
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(spec, pair_full)
    spec["receipts"] = {"pair": {"mapping_proof_clean": True}}
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(spec, pair_full)


def _v3_contract(top_k: int = 10) -> dict:
    return {
        "top_k": top_k,
        "tokenizer": ev.TOKENIZER,
        "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
        "output_unit_policy": "rank_prefix",
        "span_unit": ev.SPAN_UNIT,
    }


def _v3_capture(system: str) -> dict:
    base = {
        "system": system,
        "chunk_strategy": "whole_file" if system == "quanta" else "semble_native",
        "chunk_config": {},
        "runner_binary": {"name": f"{system}-runner", "digest": _fake_sha(f"{system}-bin")},
        "generation": 7 if system == "quanta" else 0,
        "receipt_digest": _fake_sha(f"{system}-receipt"),
        "activation_digest": _fake_sha(f"{system}-activation"),
        "model": "lex" if system == "quanta" else "potion-code-16M-v2",
        "model_revision": "r1",
    }
    base["searchd_binary"] = {"binary_digest": _fake_sha("searchd")} if system == "quanta" else None
    return base


def fixture_v3(tmp_path: Path, *, answerable_only: bool = False, blinding: str = "isolated"):
    files = {
        "a.txt": b"alpha one\nalpha two\nalpha three\nalpha four\n",
        "b.txt": b"beta one\nbeta two\n",
        "excluded.txt": b"excluded one\n",
    }
    repo, commit = _write_repo(tmp_path, files)

    def gold(path: str, start: int, end: int, grade: int | None = None):
        file_sha, block_sha, _ = _span_meta(files[path], start, end)
        start_byte, end_byte = _byte_span(files[path], start, end)
        label: dict = {
            "path": path,
            "start_byte": start_byte,
            "end_byte": end_byte,
            "start_line": start,
            "end_line": end,
            "file_sha256": file_sha,
            "block_sha256": block_sha,
        }
        if grade is not None:
            label["grade"] = grade
        return label

    def cand(path: str, start: int, end: int, rank: int):
        file_sha, block_sha, tokens = _span_meta(files[path], start, end)
        start_byte, end_byte = _byte_span(files[path], start, end)
        return {
            "path": path,
            "start_byte": start_byte,
            "end_byte": end_byte,
            "start_line": start,
            "end_line": end,
            "file_sha256": file_sha,
            "block_sha256": block_sha,
            "tokens": tokens,
            "rank": rank,
        }

    q1 = "find alpha two and beta one"
    if answerable_only:
        q2 = "find alpha three"
        tasks = [
            {
                "task_id": "T1",
                "split": "eval",
                "query": q1,
                "query_sha256": ev.digest(q1.encode()),
                "query_family_id": "fam-alpha-beta",
                "answerable": True,
                "category": "symbol",
                "gold": [gold("a.txt", 2, 2, 3), gold("b.txt", 1, 1, 1)],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": q2,
                "query_sha256": ev.digest(q2.encode()),
                "query_family_id": "fam-alpha-three",
                "answerable": True,
                "category": "semantic",
                "gold": [gold("a.txt", 3, 3, 2)],
            },
        ]
    else:
        q2 = "find the nonexistent adapter"
        tasks = [
            {
                "task_id": "T1",
                "split": "eval",
                "query": q1,
                "query_sha256": ev.digest(q1.encode()),
                "query_family_id": "fam-alpha-beta",
                "answerable": True,
                "category": "symbol",
                "gold": [gold("a.txt", 2, 2, 3), gold("b.txt", 1, 1, 1)],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": q2,
                "query_sha256": ev.digest(q2.encode()),
                "query_family_id": "fam-no-answer",
                "answerable": False,
                "gold": [],
            },
        ]
    universe_entries = [
        {"path": "a.txt", "file_sha256": ev.digest(files["a.txt"])},
        {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"])},
    ]
    suite = {
        "schema_version": 3,
        "suite_id": "fixture-v3",
        "repository_commit": commit,
        "comparison_contract": _v3_contract(),
        "routes": ["lexical", "hybrid"],
        "file_universe": universe_entries,
        "file_universe_digest": ev.universe_digest(universe_entries),
        "tasks": tasks,
    }
    _, pack, _ = ev.validate_suite(repo, suite)

    def result(task_id, route, status, spans, latency=1.5, error=None):
        return {
            "task_id": task_id,
            "route": route,
            "status": status,
            "candidates": [cand(p, s, e, i + 1) for i, (p, s, e) in enumerate(spans)],
            "timings": {"query_latency_ms": latency},
            "error": error,
        }

    if answerable_only:
        results = [
            result("T1", "lexical", "success", [("a.txt", 3, 3), ("b.txt", 1, 1)]),
            result("T1", "hybrid", "success", [("a.txt", 2, 2), ("a.txt", 3, 3), ("b.txt", 1, 1)]),
            result("T2", "lexical", "success", [("a.txt", 3, 3)]),
            result("T2", "hybrid", "success", [("a.txt", 4, 4), ("a.txt", 3, 3)]),
        ]
    else:
        results = [
            result("T1", "lexical", "success", [("a.txt", 3, 3), ("b.txt", 1, 1)]),
            result("T1", "hybrid", "success", [("a.txt", 2, 2), ("a.txt", 3, 3), ("b.txt", 1, 1)]),
            result("T2", "lexical", "success", [("a.txt", 1, 1)]),
            result("T2", "hybrid", "abstained", []),
        ]
    run = {
        "schema_version": 3,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "comparison_contract": _v3_contract(),
        "runner": {
            "name": "recorded-search-runner",
            "revision": "runner@abc",
            "run_id": "run-v3-1",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": blinding,
            "isolation_method": "separate suite access; runner cannot read suite path",
            "access_block_log": "verified EACCES on suite path for runner uid",
        },
        "captures": {"q0": _v3_capture("quanta")},
        "route_provenance": {
            "lexical": {"capture_id": "q0"},
            "hybrid": {"capture_id": "q0"},
        },
        "results": results,
    }
    suite_path = tmp_path / "suite_v3.json"
    runner_path = tmp_path / "run_v3.json"
    return repo, suite, run, suite_path, runner_path, files


def record_v3(repo, suite, run, suite_path, runner_path):
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    return ev.load_evidence(repo, suite_path, runner_path)


def test_v3_freeze_pack_carries_contract_and_is_blind(tmp_path):
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path)
    _, pack, _ = ev.validate_suite(repo, suite)
    assert pack["schema_version"] == 3
    assert pack["comparison_contract"] == suite["comparison_contract"]
    assert sorted(pack) == [
        "comparison_contract",
        "file_universe",
        "file_universe_digest",
        "repository_commit",
        "routes",
        "schema_version",
        "suite_commitment_sha256",
        "suite_id",
        "tasks",
        "tokenizer",
        "tokenizer_budget_version",
    ]
    assert pack["file_universe_digest"] == suite["file_universe_digest"]
    for task in pack["tasks"]:
        assert sorted(task) == ["query", "query_sha256", "task_id"]
    rendered = json.dumps(pack)
    assert "answerable" not in rendered and "gold" not in rendered and "grade" not in rendered


def test_v3_load_evaluate_roundtrip_and_schema_conformance(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)
    jsonschema.validate(suite, _load_schema("suite.schema.json"))
    jsonschema.validate(run, _load_schema("runner.schema.json"))
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    assert report["schema_version"] == 3
    assert report["comparison_contract"] == suite["comparison_contract"]
    assert report["captures"] == run["captures"]
    assert report["route_provenance"] == run["route_provenance"]
    assert report["sample_count"] == 2


@pytest.mark.parametrize("old_version", [1, 2, 4])
def test_current_refuses_old_or_unknown_artifact_stamps(tmp_path, old_version):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    old_suite = dict(suite, schema_version=old_version)
    with pytest.raises(ev.EvidenceError, match="unsupported suite schema"):
        ev.validate_suite(repo, old_suite)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(old_suite, _load_schema("suite.schema.json"))

    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    old_run = dict(run, schema_version=old_version)
    runner_path.write_text(json.dumps(old_run), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="unsupported runner schema"):
        ev.load_evidence(repo, suite_path, runner_path)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(old_run, _load_schema("runner.schema.json"))


def test_current_refuses_legacy_shape_even_with_current_stamp(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v3(tmp_path)
    old_suite = {
        "schema_version": ev.SCHEMA_VERSION,
        "suite_id": suite["suite_id"],
        "repository_commit": suite["repository_commit"],
        "routes": suite["routes"],
        "tasks": suite["tasks"],
    }
    with pytest.raises(ev.EvidenceError, match="missing fields"):
        ev.validate_suite(repo, old_suite)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    old_run = {
        "schema_version": ev.SCHEMA_VERSION,
        "query_pack_sha256": run["query_pack_sha256"],
        "runner": run["runner"],
        "results": run["results"],
    }
    runner_path.write_text(json.dumps(old_run), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="missing/unknown fields"):
        ev.load_evidence(repo, suite_path, runner_path)


def test_v3_refuses_unknown_fields(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)

    def attempt(mutator, match):
        mutated_suite, mutated_run = mutator(
            json.loads(json.dumps(suite)), json.loads(json.dumps(run))
        )
        suite_path.write_text(json.dumps(mutated_suite), encoding="utf-8")
        runner_path.write_text(json.dumps(mutated_run), encoding="utf-8")
        with pytest.raises(ev.EvidenceError, match=match):
            ev.load_evidence(repo, suite_path, runner_path)

    attempt(lambda s, r: (dict(s, smuggled=1), r), "missing/unknown fields")
    attempt(
        lambda s, r: (dict(s, comparison_contract=dict(s["comparison_contract"], smuggled=1)), r),
        "missing/unknown fields",
    )
    attempt(lambda s, r: (s, dict(r, smuggled=1)), "missing/unknown fields")
    attempt(
        lambda s, r: (s, dict(r, runner=dict(r["runner"], smuggled=1))), "missing/unknown fields"
    )

    def capture_extra(s, r):
        r["captures"]["q0"]["smuggled"] = 1
        return s, r

    attempt(capture_extra, "missing/unknown fields")

    def config_extra(s, r):
        r["captures"]["q0"]["chunk_config"]["smuggled"] = 1
        return s, r

    attempt(config_extra, "missing/unknown fields")

    def route_model_echo(s, r):
        # Model facts live in the capture; a route-level echo is an unknown field.
        r["route_provenance"]["lexical"]["model"] = "lex"
        return s, r

    attempt(route_model_echo, "missing/unknown fields")

    def result_extra(s, r):
        r["results"][0]["smuggled"] = 1
        return s, r

    attempt(result_extra, "missing/unknown fields")

    def timings_extra(s, r):
        r["results"][0]["timings"]["smuggled"] = 1
        return s, r

    attempt(timings_extra, "missing/unknown fields")


def test_v3_nullable_timing_semantics(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path, answerable_only=True)
    run["results"][0]["timings"]["query_latency_ms"] = None
    run["results"][1]["timings"]["query_latency_ms"] = 0
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    lexical_mean = report["budgets"]["2000"]["routes"]["lexical"]["mean_query_latency_ms"]
    assert lexical_mean == pytest.approx(1.5)
    hybrid_mean = report["budgets"]["2000"]["routes"]["hybrid"]["mean_query_latency_ms"]
    assert hybrid_mean == pytest.approx(0.75)
    rows = {(row["task_id"], row["route"]): row for row in report["per_query"]}
    assert rows[("T1", "lexical")]["query_latency_ms"] is None
    assert rows[("T1", "hybrid")]["query_latency_ms"] == 0
    with pytest.raises(ev.EvidenceError, match="timing must be a finite number"):
        ev.nullable_timing(float("inf"), "probe")
    with pytest.raises(ev.EvidenceError, match="timing must be a finite number"):
        ev.nullable_timing("1.5", "probe")
    with pytest.raises(ev.EvidenceError, match="timing must be a finite number"):
        ev.nullable_timing(-1, "probe")
    # All-null latencies report honestly instead of inventing a zero mean.
    for row in run["results"]:
        row["timings"]["query_latency_ms"] = None
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    assert (
        report["budgets"]["2000"]["routes"]["lexical"]["mean_query_latency_ms"] == "not_applicable"
    )


def test_v3_capture_reference_integrity(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)

    def attempt(mutator, match):
        mutated = json.loads(json.dumps(run))
        mutator(mutated)
        suite_path.write_text(json.dumps(suite), encoding="utf-8")
        runner_path.write_text(json.dumps(mutated), encoding="utf-8")
        with pytest.raises(ev.EvidenceError, match=match):
            ev.load_evidence(repo, suite_path, runner_path)

    def dangling(run):
        run["route_provenance"]["lexical"]["capture_id"] = "ghost"

    attempt(dangling, "unknown capture_id")
    attempt(lambda r: r.update(captures={}), "captures must be a nonempty object")

    def semble_searchd(run):
        run["captures"]["q0"] = _v3_capture("semble")
        run["captures"]["q0"]["searchd_binary"] = {"binary_digest": _fake_sha("x")}

    attempt(semble_searchd, "must be null for semble captures")

    def semble_generation(run):
        run["captures"]["q0"] = _v3_capture("semble")
        run["captures"]["q0"]["generation"] = 7

    attempt(semble_generation, "must be 0 for semble captures")

    def quanta_searchd_null(run):
        run["captures"]["q0"]["searchd_binary"] = None

    attempt(quanta_searchd_null, "must be an object")

    def legacy_syntax(run):
        run["captures"]["q0"]["chunk_strategy"] = "syntax"

    attempt(legacy_syntax, "not a frozen v3 strategy")

    def legacy_fixed_window(run):
        run["captures"]["q0"]["chunk_strategy"] = "fixed_window"

    attempt(legacy_fixed_window, "not a frozen v3 strategy")

    def bad_system(run):
        run["captures"]["q0"]["system"] = "other-engine"

    attempt(bad_system, "must be quanta or semble")

    def bad_binary_digest(run):
        run["captures"]["q0"]["runner_binary"]["digest"] = "unresolved"

    attempt(bad_binary_digest, "must be a lowercase sha256")

    def bad_receipt_digest(run):
        run["captures"]["q0"]["receipt_digest"] = "0" * 63

    attempt(bad_receipt_digest, "must be a lowercase sha256")


def test_v3_contract_binding_per_field(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path)
    # top_k drift between record and pack refuses the record.
    drifted = json.loads(json.dumps(run))
    drifted["comparison_contract"]["top_k"] = 5
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(drifted), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="comparison contract differs"):
        ev.load_evidence(repo, suite_path, runner_path)
    # Const fields cannot drift silently: any deviation fails contract validation.
    for field, value in (
        ("tokenizer", "other-tok"),
        ("tokenizer_budget_version", "qb-v9"),
        ("output_unit_policy", "rank_prefix_extended"),
        ("span_unit", "line_span_v1"),
    ):
        mutated = json.loads(json.dumps(run))
        mutated["comparison_contract"][field] = value
        runner_path.write_text(json.dumps(mutated), encoding="utf-8")
        with pytest.raises(ev.EvidenceError, match="record.comparison_contract"):
            ev.load_evidence(repo, suite_path, runner_path)
    # The suite side is pinned too: a doctored suite changes the pack,
    # so the pack binding fires before contract comparison is even reached.
    mutated_suite = json.loads(json.dumps(suite))
    mutated_suite["comparison_contract"]["top_k"] = 5
    suite_path.write_text(json.dumps(mutated_suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="query pack hash mismatch"):
        ev.load_evidence(repo, suite_path, runner_path)


def _g0_spec() -> dict:
    return {
        "repo": "/tmp/repo",
        "manifest": "/tmp/manifest.json",
        "suite": "/tmp/suite.json",
        "query_pack": "/tmp/pack.json",
        "top_k": 10,
        "output_root": "/tmp/out",
        "runner_binary": "/tmp/runner",
        "strategies": [{"name": "whole_file"}],
        "searchd_binary": "/tmp/searchd",
        "searchd_expected_sha256": _fake_sha("searchd"),
        "semble_python": "/tmp/venv/bin/python",
        "semble_lockfile": "/tmp/semble-lock.txt",
        "semble_lockfile_sha256": _fake_sha("lock"),
        "cache_regime": "true_process_cold",
        "claims": {"quality": False, "speed": False, "same_model": False, "incremental": False},
    }


def test_v3_pair_spec_schema():
    schema = _load_schema("pair-spec.schema.json")
    jsonschema.validate(_g0_spec(), schema)

    def invalid(mutator):
        spec = json.loads(json.dumps(_g0_spec()))
        mutator(spec)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(spec, schema)

    invalid(lambda s: s.update(runner_revision="derived-is-forbidden"))
    invalid(lambda s: s["claims"].update(speed="false"))
    invalid(lambda s: s.pop("searchd_expected_sha256"))
    invalid(lambda s: s["strategies"].append({"name": "syntax"}))
    invalid(lambda s: s.update(evidence={"pair": {"mapping_proof_clean": True}}))
    invalid(lambda s: s.update(top_k=0))
    invalid(lambda s: s.update(semble_lockfile=""))
    invalid(lambda s: s.update(cache_regime="lukewarm"))
    invalid(lambda s: s.update(semble_repetitions=2))
    invalid(lambda s: s.update(semble_warmup_passes=0))

    qualified = _g0_spec()
    qualified["scope"] = "qualified"
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(qualified, schema)
    qualified["admission"] = {
        "manifest": "/tmp/admission.json",
        "license_receipt": "/tmp/license.json",
        "annotation_receipts": ["/tmp/a.json", "/tmp/b.json"],
        "adjudication_receipt": "/tmp/adjudication.json",
    }
    jsonschema.validate(qualified, schema)


def test_v3_spec_accepts_lockfile_path(tmp_path):
    spec_path = tmp_path / "spec.json"
    spec_path.write_text(json.dumps(_g0_spec()), encoding="utf-8")
    loaded = pairrun.load_spec(spec_path)
    assert loaded["semble_lockfile"] == "/tmp/semble-lock.txt"
    bad = _g0_spec()
    bad["semble_lockfile"] = ""
    spec_path.write_text(json.dumps(bad), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="spec.semble_lockfile must be a nonempty string"):
        pairrun.load_spec(spec_path)
    bad = _g0_spec()
    bad["cache_regime"] = "lukewarm"
    spec_path.write_text(json.dumps(bad), encoding="utf-8")
    with pytest.raises(pairrun.RunError, match="spec.cache_regime must be"):
        pairrun.load_spec(spec_path)
    for removed_key in ("semble_repetitions", "semble_warmup_passes"):
        bad = _g0_spec()
        bad[removed_key] = 2
        spec_path.write_text(json.dumps(bad), encoding="utf-8")
        with pytest.raises(pairrun.RunError, match="unknown keys"):
            pairrun.load_spec(spec_path)


def test_retrieval_recipes_download_nothing():
    # T14: benchmark recipes never implicitly fetch models, packages or
    # repos; every retrieval/benchmark-prep recipe body is scanned, so a
    # future recipe that downloads fails this test too.
    root = Path(pairrun.__file__).resolve().parents[3]
    bodies = {}
    current = None
    for line in (root / "Justfile").read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        is_header = (
            line
            and not line[0].isspace()
            and stripped.endswith(":")
            and ":=" not in line
            and not stripped.startswith(("#", "set ", "export ", "import "))
        )
        if is_header:
            current = stripped[:-1].split()[0]
            bodies[current] = []
        elif current is not None:
            bodies[current].append(line)
    targets = [
        name for name in bodies if name.startswith("retrieval") or name.startswith("benchmark-prep")
    ]
    assert "retrieval-sdk-proof" in targets
    assert "benchmark-prep-local" in targets
    verbs = (
        "pip install",
        "pip download",
        "uv pip",
        "curl ",
        "wget ",
        "cargo install",
        "git clone",
        "snapshot_download",
        "huggingface_hub",
    )
    for name in targets:
        text = "\n".join(bodies[name]).lower()
        for verb in verbs:
            assert verb not in text, f"{name} downloads via {verb}"


def test_benchmark_prep_does_not_repeat_retrieval_contracts():
    root = Path(pairrun.__file__).resolve().parents[3]

    def recipe(*args: str) -> str:
        completed = subprocess.run(
            ["just", "--dry-run", *args],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
        )
        return completed.stdout + completed.stderr

    prep = recipe("benchmark-prep-local")
    proof = recipe("retrieval-contract-proof", "/tmp/retrieval-proof")
    local = recipe("retrieval-contract-local")
    assert "test_retrieval_benchmark.py -q" not in prep
    assert "test -p quanta-index-retrieval-bench" not in prep
    assert "test_retrieval_benchmark.py -q" in proof
    assert "--test chunking_contract" in proof
    assert "test_retrieval_benchmark.py -q" in local
    assert "--test chunking_contract" in local


def test_retrieval_verdict_recipe_matches_cli_parser():
    root = Path(pairrun.__file__).resolve().parents[3]
    values = ("/tmp/repo", "/tmp/suite.json", "/tmp/run-manifest.json", "/tmp/verdict.json")
    completed = subprocess.run(
        ["just", "--dry-run", "retrieval-verdict", *values],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    rendered = completed.stderr.strip() or completed.stdout.strip()
    command = shlex.split(rendered)
    assert command[:2] == ["python3", "tools/benchmark/retrieval/run.py"]
    parsed = pairrun.build_parser().parse_args(command[2:])
    assert (parsed.command, parsed.repo, parsed.suite, parsed.run_manifest, parsed.out) == (
        "verdict",
        *values,
    )


def _g0_manifest() -> dict:
    return {
        "manifest_version": 1,
        "blinding": "attested",
        "isolation_method": "m",
        "access_block_log": "l",
        "scope": "exploratory",
        "claims": {"quality": False, "speed": False, "same_model": False, "incremental": False},
        "repetitions": 1,
        "evidence": {
            "pair": {"mapping_proof_digest": _fake_sha("mapping")},
            "perf": {
                "observations_floor": 0,
                "fresh_roots": 1,
                "phase_boundaries": False,
                "resource_accounting": True,
            },
        },
        "host": {
            "start_digest": _fake_sha("hs"),
            "end_digest": _fake_sha("he"),
            "cache_regime": "true_process_cold",
        },
        "artifacts": {
            "suite": "suite.json",
            "query_pack": "pack.json",
            "corpus_manifest": "manifest.json",
            "mapping_proof": "mapping-proof.json",
            "latency_matrix": "latency-matrix.json",
            "host_start": "host-start.json",
            "host_end": "host-end.json",
            "host_profile": "host-profile.json",
            "records": ["lex.json", "sem.json"],
            "reports": [],
            "quanta_manifests": [],
            "semble_adapter_manifest": "adapter-manifest.json",
            "semble_lockfile": "lockfile.txt",
            "semble_native": ["native.json"],
            "phase_metrics": ["phase.json"],
            "resource_metrics": ["resource.json"],
            "protocol_lock": "protocol-lock.json",
        },
        "provenance": {
            "quanta": {
                "source_sha": "a" * 40,
                "source_closure_digest": None,
                "binary_digest": _fake_sha("qb"),
                "embedder": "potion-code",
            },
            "semble": {
                "revision": "0.6.0",
                "lockfile_digest": _fake_sha("lock"),
                "interpreter_digest": _fake_sha("py"),
                "model_asset_digest": _fake_sha("m"),
            },
            "corpus": {"digest": _fake_sha("c"), "path_sha_diff_digest": _fake_sha("d")},
            "suite": {
                "suite_digest": _fake_sha("s"),
                "query_pack_digest": _fake_sha("p"),
                "tokenizer_budget_version": "qb-v1",
            },
            "host": {"profile_digest": _fake_sha("hp"), "check_record_digest": _fake_sha("h")},
            "admission": {"manifest_digest": None},
        },
    }


def test_v3_manifest_schema():
    schema = _load_schema("run-manifest.schema.json")
    jsonschema.validate(_g0_manifest(), schema)

    def invalid(mutator):
        manifest = json.loads(json.dumps(_g0_manifest()))
        mutator(manifest)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(manifest, schema)

    invalid(lambda m: m["claims"].update(speed="false"))
    invalid(lambda m: m.update(smuggled=True))
    invalid(lambda m: m["evidence"].pop("pair"))
    invalid(lambda m: m["provenance"]["semble"].update(revision="0.7.0"))
    invalid(lambda m: m["provenance"]["quanta"].update(source_sha="unresolved"))
    invalid(lambda m: m["provenance"]["quanta"].pop("source_closure_digest"))
    invalid(lambda m: m["provenance"]["quanta"].update(source_closure_digest=_fake_sha("x")))
    invalid(lambda m: m["provenance"]["quanta"].update(embedder="openai"))
    invalid(lambda m: m["provenance"]["quanta"].pop("embedder"))
    invalid(lambda m: m["host"].update(cache_regime="lukewarm"))
    invalid(lambda m: m["host"].pop("cache_regime"))

    qualified = json.loads(json.dumps(_g0_manifest()))
    qualified["scope"] = "qualified"
    qualified["artifacts"].update(
        {
            "admission_manifest": "admission.json",
            "license_receipt": "license.json",
            "annotation_receipts": ["annotation-a.json", "annotation-b.json"],
            "adjudication_receipt": "adjudication.json",
            "driver_source_closure": "driver-source-closure.json",
        }
    )
    qualified["provenance"]["admission"]["manifest_digest"] = _fake_sha("admission")
    qualified["provenance"]["quanta"]["source_closure_digest"] = _fake_sha("closure")
    jsonschema.validate(qualified, schema)
    for mutator in (
        lambda value: value["artifacts"].pop("driver_source_closure"),
        lambda value: value["provenance"]["quanta"].update(source_closure_digest=None),
        lambda value: value["provenance"]["admission"].update(manifest_digest=None),
    ):
        mutant = json.loads(json.dumps(qualified))
        mutator(mutant)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(mutant, schema)


def _g0_verdict() -> dict:
    states = ["CONTRACT_GREEN", "SDK_PATH_GREEN", "PAIR_VALID", "PERF_QUALIFIED", "QUALITY_DELTA"]
    return {
        "verdict_version": 2,
        "states": {name: "not_run" for name in states},
        "state_evidence": {
            name: {"reason": "no_evidence", "proof_digest": None} for name in states
        },
        "blinding": "attested",
        "isolation_method": "m",
        "access_block_log": "l",
        "missing_t_ids": ["T00"],
        "not_applicable_t_ids": ["T15", "T16"],
        "failure_class": "none",
        "provenance": {
            "quanta": {
                "source_sha": "a" * 40,
                "binary_digest": _fake_sha("qb"),
                "embedder": "potion-code",
            },
            "semble": {"revision": "0.6.0", "lockfile_digest": _fake_sha("lock")},
            "corpus": {"digest": _fake_sha("c"), "path_sha_diff_digest": _fake_sha("d")},
            "suite": {
                "suite_digest": _fake_sha("s"),
                "query_pack_digest": _fake_sha("p"),
                "tokenizer_budget_version": "qb-v1",
            },
            "host": {"profile_digest": _fake_sha("hp"), "check_record_digest": _fake_sha("h")},
            "admission": {"manifest_digest": None},
        },
        "counts": {"selected": 0, "executed": 0, "passed": 0, "failed": 0},
        "comparisons": [
            {
                "strategy": "whole_file",
                "baseline_route": "semble-hybrid",
                "candidate_route": "lexical",
                "primary_metric": "recall_at_10",
                "primary_delta": 0.0,
                "record_digest": _fake_sha("rec"),
                "report_digest": _fake_sha("rep"),
            },
        ],
    }


def test_v3_verdict_schema():
    schema = _load_schema("verdict.schema.json")
    jsonschema.validate(_g0_verdict(), schema)

    def invalid(mutator):
        verdict = json.loads(json.dumps(_g0_verdict()))
        mutator(verdict)
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(verdict, schema)

    invalid(lambda v: v.update(verdict_version=1))
    invalid(lambda v: v.update(primary_delta=0.0))
    invalid(lambda v: v["comparisons"][0].pop("primary_delta"))
    invalid(lambda v: v["states"].update(PAIR_VALID="maybe"))
    invalid(lambda v: v["provenance"].update(quanta_source_sha="a" * 40))
    invalid(lambda v: v.update(failure_class="typo"))
    invalid(lambda v: v["provenance"]["quanta"].update(embedder="openai"))


# --- Item 1: byte spans, universe binding, split custody (strict §1) ---


def _repack(repo, suite, run):
    _, pack, _ = ev.validate_suite(repo, suite)
    run = json.loads(json.dumps(run))
    run["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    return pack, run


def _v3_block(files, path, start, end, *, tokens=None, rank=None, grade=None):
    file_sha, block_sha, counted = _span_meta(files[path], start, end)
    start_byte, end_byte = _byte_span(files[path], start, end)
    item = {
        "path": path,
        "start_byte": start_byte,
        "end_byte": end_byte,
        "start_line": start,
        "end_line": end,
        "file_sha256": file_sha,
        "block_sha256": block_sha,
    }
    if tokens is not None:
        item["tokens"] = counted
    if rank is not None:
        item["rank"] = rank
    if grade is not None:
        item["grade"] = grade
    return item


def test_v3_byte_span_verification(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)

    def attempt(mutator, match):
        mutated = json.loads(json.dumps(run))
        mutator(mutated["results"][0]["candidates"][0])
        suite_path.write_text(json.dumps(suite), encoding="utf-8")
        runner_path.write_text(json.dumps(mutated), encoding="utf-8")
        with pytest.raises(ev.EvidenceError, match=match):
            ev.load_evidence(repo, suite_path, runner_path)

    attempt(lambda c: c.update(end_byte=c["end_byte"] - 1), "byte span disagrees")
    attempt(lambda c: c.update(start_byte=c["start_byte"] + 1), "byte span disagrees")
    attempt(lambda c: c.update(end_byte=10**9), "byte span runs past EOF")
    attempt(lambda c: c.update(start_byte=c["end_byte"]), "empty byte span")
    attempt(lambda c: c.update(start_byte=-1), "must be an integer >= 0")

    mutated_suite = json.loads(json.dumps(suite))
    mutated_suite["tasks"][0]["gold"][0]["end_byte"] += 5
    with pytest.raises(ev.EvidenceError, match="byte span disagrees"):
        ev.validate_suite(repo, mutated_suite)

    # A byte span cutting a UTF-8 boundary is refused, not decoded lossily.
    repo2, commit2 = _write_repo(tmp_path / "uni", {"u.txt": "aé\nb\n".encode()})
    source = ev.SourceSnapshot(repo2, commit2)
    bad = {
        "path": "u.txt",
        "start_byte": 0,
        "end_byte": 2,
        "start_line": 1,
        "end_line": 1,
        "file_sha256": ev.digest("aé\nb\n".encode()),
        "block_sha256": "0" * 64,
        "tokens": 1,
        "rank": 1,
    }
    with pytest.raises(ev.EvidenceError, match="cuts a UTF-8 boundary"):
        ev.block(source, bad, "probe", candidate=True)
    good = dict(bad, end_byte=4, block_sha256=ev.digest("aé\n".encode()), tokens=2)
    assert ev.block(source, good, "probe", candidate=True)["tokens"] == 2


def test_v3_byte_coverage_decides_credit():
    gold = {"path": "a", "start_byte": 10, "end_byte": 20, "start_line": 2, "end_line": 2}
    same = dict(gold)
    assert ev.covers(same, gold) is True
    subset = dict(gold, start_byte=12, end_byte=15)
    assert ev.covers(subset, gold) is False
    superset = dict(gold, start_byte=5, end_byte=25)
    assert ev.covers(superset, gold) is True
    shifted = dict(gold, start_byte=15, end_byte=25)
    assert ev.covers(shifted, gold) is False
    assert ev.covers(dict(gold, path="b"), gold) is False


def test_v3_partial_bytes_earn_no_credit(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v3(tmp_path, answerable_only=True)
    suite["tasks"][0]["gold"] = [_v3_block(files, "a.txt", 2, 3, grade=3)]
    run["results"][0]["candidates"] = [_v3_block(files, "a.txt", 2, 2, tokens=True, rank=1)]
    _pack, run = _repack(repo, suite, run)
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    rows = {(r["task_id"], r["route"]): r for r in report["per_query"]}
    assert rows[("T1", "lexical")]["chunk_recall_at_10"] == 0.0
    run["results"][0]["candidates"] = [_v3_block(files, "a.txt", 2, 3, tokens=True, rank=1)]
    _pack, run = _repack(repo, suite, run)
    loaded_suite, pack, loaded_run = record_v3(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(loaded_suite, pack, loaded_run, "lexical", "hybrid")
    rows = {(r["task_id"], r["route"]): r for r in report["per_query"]}
    assert rows[("T1", "lexical")]["chunk_recall_at_10"] == 1.0


def test_v3_universe_binding(tmp_path):
    repo, suite, _run, _sp, _rp, _files = fixture_v3(tmp_path)
    mutated = json.loads(json.dumps(suite))
    mutated["file_universe_digest"] = "0" * 64
    with pytest.raises(ev.EvidenceError, match="file universe digest mismatch"):
        ev.validate_suite(repo, mutated)
    mutated = json.loads(json.dumps(suite))
    del mutated["file_universe"]
    with pytest.raises(ev.EvidenceError, match="missing fields"):
        ev.validate_suite(repo, mutated)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(mutated, _load_schema("suite.schema.json"))


def _train_probe_task(files, task_id, query, family, gold_span):
    return {
        "task_id": task_id,
        "split": "train",
        "query": query,
        "query_sha256": ev.digest(query.encode()),
        "query_family_id": family,
        "answerable": True,
        "gold": [_v3_block(files, *gold_span, grade=2)],
    }


def test_v3_query_family_split_separation(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path)
    mutated = json.loads(json.dumps(suite))
    mutated["tasks"].append(
        _train_probe_task(
            files, "T0", "find alpha training probe", "fam-alpha-beta", ("a.txt", 1, 1)
        )
    )
    with pytest.raises(ev.EvidenceError, match="query family spans train and eval"):
        ev.validate_suite(repo, mutated)
    mutated["tasks"][-1]["query_family_id"] = "fam-train-only"
    ev.validate_suite(repo, mutated)


def test_v3_query_near_duplicates_refused(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path)
    mutated = json.loads(json.dumps(suite))
    mutated["tasks"].append(
        _train_probe_task(
            files, "T0", "FIND  alpha TWO and BETA one!!", "fam-train-only", ("a.txt", 1, 1)
        )
    )
    with pytest.raises(ev.EvidenceError, match="normalized match"):
        ev.validate_suite(repo, mutated)
    mutated["tasks"][-1]["query"] = "find alpha two and beta one ok"
    mutated["tasks"][-1]["query_sha256"] = ev.digest(mutated["tasks"][-1]["query"].encode())
    with pytest.raises(ev.EvidenceError, match="near-duplicate"):
        ev.validate_suite(repo, mutated)


def test_v3_leakage_allowlist(tmp_path):
    repo, suite, _run, _sp, _rp, files = fixture_v3(tmp_path, answerable_only=True)
    mutated = json.loads(json.dumps(suite))
    mutated["tasks"][1]["split"] = "train"
    mutated["tasks"][1]["gold"] = [_v3_block(files, "a.txt", 2, 2, grade=2)]
    with pytest.raises(ev.EvidenceError, match="leakage across train/eval split"):
        ev.validate_suite(repo, mutated)
    mutated["leakage_allowlist"] = [
        {
            "path": "a.txt",
            "start_line": 3,
            "end_line": 3,
            "rationale_digest": _fake_sha("rationale"),
        },
    ]
    with pytest.raises(ev.EvidenceError, match="leakage across train/eval split"):
        ev.validate_suite(repo, mutated)
    mutated["leakage_allowlist"] = [
        {
            "path": "a.txt",
            "start_line": 2,
            "end_line": 2,
            "rationale_digest": _fake_sha("rationale"),
        },
    ]
    ev.validate_suite(repo, mutated)
    mutated["leakage_allowlist"][0]["rationale_digest"] = "zzz"
    with pytest.raises(ev.EvidenceError, match="must be a lowercase sha256"):
        ev.validate_suite(repo, mutated)


def test_v3_timeout_requires_measured_duration(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path, answerable_only=True)
    mutated = json.loads(json.dumps(run))
    row = mutated["results"][0]
    row["status"] = "timeout"
    row["candidates"] = []
    row["timings"] = {"query_latency_ms": None}
    row["error"] = {"code": "deadline", "message": "timed out"}
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(mutated), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="measured duration"):
        ev.load_evidence(repo, suite_path, runner_path)
    row["timings"] = {"query_latency_ms": 5.0}
    runner_path.write_text(json.dumps(mutated), encoding="utf-8")
    ev.load_evidence(repo, suite_path, runner_path)
    row["status"] = "error"
    row["timings"] = {"query_latency_ms": None}
    runner_path.write_text(json.dumps(mutated), encoding="utf-8")
    ev.load_evidence(repo, suite_path, runner_path)


def test_v3_duplicate_candidate_byte_span_refused(tmp_path):
    repo, suite, run, suite_path, runner_path, _files = fixture_v3(tmp_path, answerable_only=True)
    mutated = json.loads(json.dumps(run))
    first = dict(mutated["results"][0]["candidates"][0], rank=3)
    mutated["results"][0]["candidates"].append(first)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(mutated), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="duplicate candidate byte span"):
        ev.load_evidence(repo, suite_path, runner_path)


# --- Cross-language fixtures (strict §1 independent oracles) ---

FIXTURES_DIR = Path(ev.__file__).resolve().parent / "fixtures"


def _load_fixture(name: str):
    return json.loads((FIXTURES_DIR / name).read_text(encoding="utf-8"))


def test_fixture_canonical_json_vectors():
    vectors = _load_fixture("canonical-json-vectors.json")
    assert len(vectors) >= 5
    for vector in vectors:
        assert ev.canonical(vector["value"]).decode("utf-8") == vector["canonical"], vector["name"]


def test_fixture_tokenizer_vectors():
    vectors = _load_fixture("tokenizer-vectors.json")
    assert len(vectors) >= 5
    for vector in vectors:
        found = ev.TOKEN_RE.findall(vector["text"])
        assert found == vector["tokens"], vector["text"]
        assert len(found) == vector["count"], vector["text"]


def test_fixture_span_vectors(tmp_path):
    vectors = _load_fixture("span-vectors.json")
    assert len(vectors) >= 5
    files = {f"{v['name']}.txt": v["file"].encode("utf-8") for v in vectors}
    repo, commit = _write_repo(tmp_path, files)
    source = ev.SourceSnapshot(repo, commit)
    for vector in vectors:
        name = f"{vector['name']}.txt"
        raw = files[name]
        item = {
            "path": name,
            "start_byte": vector["start_byte"],
            "end_byte": vector["end_byte"],
            "start_line": vector["start_line"],
            "end_line": vector["end_line"],
            "file_sha256": ev.digest(raw),
            "block_sha256": vector["block_sha256"],
            "tokens": vector["tokens"],
            "rank": 1,
        }
        checked = ev.block(source, item, "fixture " + vector["name"], candidate=True)
        assert checked["tokens"] == vector["tokens"]
        if item["end_byte"] < len(raw):
            mutated = dict(
                item,
                end_byte=item["end_byte"] + 1,
                block_sha256=ev.digest(raw[item["start_byte"] : item["end_byte"] + 1]),
            )
            match = "byte span disagrees"
        else:
            mutated = dict(item, end_byte=item["end_byte"] + 1)
            match = "byte span runs past EOF"
        with pytest.raises(ev.EvidenceError, match=match):
            ev.block(source, mutated, "fixture " + vector["name"], candidate=True)


def test_fixture_split_leakage_mutants():
    mutants = _load_fixture("split-leakage-mutants.json")
    assert len(mutants) >= 5
    for mutant in mutants:
        labels = {
            "train": {tuple(span) for span in mutant["train"]},
            "eval": {tuple(span) for span in mutant["eval"]},
        }
        allowlist = frozenset(tuple(span) for span in mutant["allowlist"])
        if mutant["expect_ok"]:
            ev._check_split_leakage(labels, allowlist)
        else:
            with pytest.raises(ev.EvidenceError, match="leakage across train/eval split"):
                ev._check_split_leakage(labels, allowlist)
