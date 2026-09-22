"""Real Git source, blinded runner boundary and budgeted route scoring."""

from __future__ import annotations

import json
import math
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator as ev


def fixture(tmp_path: Path):
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
        "start_line": 1,
        "end_line": 1,
        "file_sha256": ev.digest(source),
        "block_sha256": ev.digest(source),
    }
    query = "Find the source block implementing token handling"
    empty_query = "Find the nonexistent adapter"
    suite = {
        "schema_version": 1,
        "suite_id": "fixture-1",
        "repository_commit": commit,
        "routes": ["lexical", "hybrid"],
        "tasks": [
            {
                "task_id": "T1",
                "split": "eval",
                "query": query,
                "query_sha256": ev.digest(query.encode()),
                "answerable": True,
                "gold": [label],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": empty_query,
                "query_sha256": ev.digest(empty_query.encode()),
                "answerable": False,
                "gold": [],
            },
        ],
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    candidate = dict(label, tokens=3000)
    run = {
        "schema_version": 1,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "runner": {
            "name": "recorded-search-runner",
            "revision": "runner@abc",
            "run_id": "run-1",
            "tokenizer": ev.TOKENIZER,
            "gold_access": False,
        },
        "results": [
            {"task_id": "T1", "route": "lexical", "abstain": True, "candidates": []},
            {"task_id": "T1", "route": "hybrid", "abstain": False, "candidates": [candidate]},
            {"task_id": "T2", "route": "lexical", "abstain": False, "candidates": [candidate]},
            {"task_id": "T2", "route": "hybrid", "abstain": True, "candidates": []},
        ],
    }
    suite_path = tmp_path / "suite.json"
    runner_path = tmp_path / "run.json"
    return repo, suite, run, suite_path, runner_path


def record(repo, suite, run, suite_path, runner_path):
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    return ev.load_evidence(repo, suite_path, runner_path)


def test_budgeted_bcy_abstention_and_paired_ablation(tmp_path):
    repo, suite, run, suite_path, runner_path = fixture(tmp_path)
    loaded = record(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid")
    small = report["budgets"]["2000"]
    large = report["budgets"]["4000"]
    assert small["routes"]["hybrid"]["bcy"] == 0
    assert small["routes"]["hybrid"]["no_gold_abstention"] == 1
    assert large["routes"]["hybrid"]["bcy"] == 1
    assert large["routes"]["lexical"]["bcy"] == 0
    assert large["comparison"]["paired_wins"] == 2
    assert large["comparison"]["paired_losses"] == 0
    assert set(report["budgets"]) == {"2000", "4000", "8000", "16000"}
    assert "gold" not in json.dumps(loaded[1])


@pytest.mark.parametrize(
    "mutation,match",
    [
        (lambda s, r: s["tasks"][0].update(query_sha256="0" * 64), "query hash mismatch"),
        (
            lambda s, r: s["tasks"][0]["gold"][0].update(block_sha256="0" * 64),
            "block hash mismatch",
        ),
        (lambda s, r: s["tasks"][0]["gold"][0].update(start_line=2), "inverted line span"),
        (lambda s, r: s["tasks"][1].update(answerable=True), "answerable/gold mismatch"),
        (
            lambda s, r: s["tasks"][1].update(
                query=s["tasks"][0]["query"], query_sha256=s["tasks"][0]["query_sha256"]
            ),
            "query leakage",
        ),
        (lambda s, r: r["runner"].update(gold_access=True), "gold access"),
        (lambda s, r: r["results"][1]["candidates"][0].update(tokens=1), "token count mismatch"),
        (
            lambda s, r: r["results"][1]["candidates"][0].update(file_sha256="0" * 64),
            "file hash mismatch",
        ),
        (
            lambda s, r: r["results"][1]["candidates"][0].update(path="../target.txt"),
            "unsafe repository path",
        ),
        (lambda s, r: r["results"][1]["candidates"][0].update(gold=True), "unknown fields"),
        (lambda s, r: r["results"].pop(), "missing task route evidence"),
        (lambda s, r: r.update(query_pack_sha256="0" * 64), "query pack hash mismatch"),
        (lambda s, r: s.update(suite_id="changed"), "query pack hash mismatch"),
        (lambda s, r: s.update(repository_commit="0" * 40), "checkout HEAD differs"),
    ],
)
def test_fails_closed_on_invalid_evidence(tmp_path, mutation, match):
    repo, suite, run, suite_path, runner_path = fixture(tmp_path)
    mutation(suite, run)
    with pytest.raises(ev.EvidenceError, match=match):
        record(repo, suite, run, suite_path, runner_path)


def test_dirty_checkout_refused(tmp_path):
    repo, suite, run, suite_path, runner_path = fixture(tmp_path)
    (repo / "untracked.txt").write_text("new", encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="untracked changes"):
        record(repo, suite, run, suite_path, runner_path)


def test_gold_block_reused_across_splits_refused(tmp_path):
    repo, suite, run, suite_path, runner_path = fixture(tmp_path)
    train = dict(suite["tasks"][0], task_id="TRAIN", split="train", query="training lookup")
    train["query_sha256"] = ev.digest(train["query"].encode())
    suite["tasks"].append(train)
    with pytest.raises(ev.EvidenceError, match="gold label leakage"):
        record(repo, suite, run, suite_path, runner_path)


def test_label_must_be_tracked_source(tmp_path):
    repo, suite, run, suite_path, runner_path = fixture(tmp_path)
    (repo / "ignored.txt").write_text("ignored\n", encoding="utf-8")
    (repo / ".git" / "info" / "exclude").write_text("ignored.txt\n", encoding="utf-8")
    source = b"ignored\n"
    suite["tasks"][0]["gold"][0] = {
        "path": "ignored.txt",
        "start_line": 1,
        "end_line": 1,
        "file_sha256": ev.digest(source),
        "block_sha256": ev.digest(source),
    }
    with pytest.raises(ev.EvidenceError, match="not tracked"):
        record(repo, suite, run, suite_path, runner_path)


def test_duplicate_json_keys_refused(tmp_path):
    path = tmp_path / "duplicate.json"
    path.write_text('{"schema_version":1,"schema_version":1}', encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="duplicate JSON key"):
        ev.read_json(path)


def test_cli_requires_recorded_runner(tmp_path, capsys):
    repo, suite, run, suite_path, runner_path = fixture(tmp_path)
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


def test_cli_writes_verifiable_pack_and_report(tmp_path, capsys):
    repo, suite, run, suite_path, runner_path = fixture(tmp_path)
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
    assert "gold" not in pack_path.read_text(encoding="utf-8")
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
    assert report["budgets"]["4000"]["comparison"]["paired_wins"] == 2


def test_rank_prefix_does_not_skip_oversize_candidate():
    rows = [{"tokens": 3000}, {"tokens": 10}]
    assert ev.selected(rows, 2000) == ([], 0)


def test_token_unit_uses_explicit_ascii_ranges():
    assert ev.TOKEN_RE.findall("alpha_1 한글 !") == ["alpha_1", "한", "글", "!"]


# --- RB-01 v2 suite and scoring ---

def _write_repo(tmp_path: Path, files: dict[str, bytes]) -> tuple[Path, str]:
    repo = tmp_path / "source_v2"
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


def fixture_v2(tmp_path: Path, *, answerable_only: bool = False, blinding: str = "isolated"):
    files = {
        "a.txt": b"alpha one\nalpha two\nalpha three\nalpha four\n",
        "b.txt": b"beta one\nbeta two\n",
        "excluded.txt": b"excluded one\n",
    }
    repo, commit = _write_repo(tmp_path, files)

    def gold(path: str, start: int, end: int, grade: int | None = None):
        file_sha, block_sha, _ = _span_meta(files[path], start, end)
        label: dict = {
            "path": path,
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
        return {
            "path": path,
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
                "answerable": True,
                "category": "symbol",
                "gold": [gold("a.txt", 2, 2, 3), gold("b.txt", 1, 1, 1)],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": q2,
                "query_sha256": ev.digest(q2.encode()),
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
                "answerable": True,
                "category": "symbol",
                "gold": [gold("a.txt", 2, 2, 3), gold("b.txt", 1, 1, 1)],
            },
            {
                "task_id": "T2",
                "split": "eval",
                "query": q2,
                "query_sha256": ev.digest(q2.encode()),
                "answerable": False,
                "gold": [],
            },
        ]
    suite = {
        "schema_version": 2,
        "suite_id": "fixture-v2",
        "repository_commit": commit,
        "routes": ["lexical", "hybrid"],
        "file_universe": [
            {"path": "a.txt", "file_sha256": ev.digest(files["a.txt"])},
            {"path": "b.txt", "file_sha256": ev.digest(files["b.txt"])},
        ],
        "tasks": tasks,
    }
    _, pack, _ = ev.validate_suite(repo, suite)

    def result(task_id: str, route: str, status: str, spans: list[tuple[str, int, int]], latency: float = 1.5, error=None):
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
        "schema_version": 2,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "runner": {
            "name": "recorded-search-runner",
            "revision": "runner@abc",
            "run_id": "run-v2-1",
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": blinding,
            "isolation_method": "separate suite access; runner cannot read suite path",
            "access_block_log": "verified EACCES on suite path for runner uid",
        },
        "route_provenance": {
            "lexical": {"system": "quanta", "model": "lex", "model_revision": "r1"},
            "hybrid": {"system": "quanta", "model": "hybrid", "model_revision": "r1"},
        },
        "results": results,
    }
    suite_path = tmp_path / "suite_v2.json"
    runner_path = tmp_path / "run_v2.json"
    return repo, suite, run, suite_path, runner_path, files


def record_v2(repo, suite, run, suite_path, runner_path):
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    return ev.load_evidence(repo, suite_path, runner_path)


def test_v2_freeze_pack_is_blind(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v2(tmp_path)
    _, pack, _ = ev.validate_suite(repo, suite)
    text = json.dumps(pack)
    assert "gold" not in text
    assert "grade" not in text
    assert "answerable" not in text
    assert pack["schema_version"] == 2
    assert pack["tokenizer_budget_version"] == ev.TOKENIZER_BUDGET_VERSION
    assert pack["file_universe"] == suite["file_universe"]
    assert all(set(t) == {"task_id", "query", "query_sha256"} for t in pack["tasks"])


def test_v2_hand_calculated_rank_metrics(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v2(tmp_path)
    loaded = record_v2(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid")
    assert report["schema_version"] == 2
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
    assert report["rank_metrics"]["comparison"]["primary_delta_ci_95"]["status"] == ev.NOT_APPLICABLE


def test_ndcg_credits_each_gold_span_once_even_when_chunks_overlap():
    label = {"path": "src/lib.rs", "start_line": 3, "end_line": 3, "grade": 3}
    candidates = [
        {"path": "src/lib.rs", "start_line": 1, "end_line": 3},
        {"path": "src/lib.rs", "start_line": 2, "end_line": 4},
    ]
    assert ev.ndcg_at_k(candidates, [label], 10) == pytest.approx(1.0)
    assert ev.ndcg_at_k(list(reversed(candidates)), [label], 10) == pytest.approx(1.0)


def test_v2_bcy_budget_prefix_and_out_of_budget_not_credited(tmp_path):
    repo = tmp_path / "source"
    repo.mkdir()
    source = ("token " * 3000 + "\n").encode()
    (repo / "target.txt").write_bytes(source)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "target.txt"], check=True)
    subprocess.run(
        ["git", "-C", str(repo), "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
         "-c", "commit.gpgsign=false", "commit", "-qm", "frozen"],
        check=True,
    )
    commit = ev.git(repo, "rev-parse", "HEAD")
    label = {
        "path": "target.txt", "start_line": 1, "end_line": 1,
        "file_sha256": ev.digest(source), "block_sha256": ev.digest(source), "grade": 2,
    }
    q1 = "find token handling"
    q2 = "find nonexistent"
    suite = {
        "schema_version": 2, "suite_id": "budget-v2", "repository_commit": commit,
        "routes": ["lexical", "hybrid"],
        "file_universe": [{"path": "target.txt", "file_sha256": ev.digest(source)}],
        "tasks": [
            {"task_id": "T1", "split": "eval", "query": q1,
             "query_sha256": ev.digest(q1.encode()), "answerable": True, "gold": [label]},
            {"task_id": "T2", "split": "eval", "query": q2,
             "query_sha256": ev.digest(q2.encode()), "answerable": False, "gold": []},
        ],
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    cand = dict({k: v for k, v in label.items() if k != "grade"}, tokens=3000, rank=1)
    run = {
        "schema_version": 2, "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "runner": {
            "name": "r", "revision": "r@1", "run_id": "run-1", "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION, "gold_access": False,
            "blinding": "isolated", "isolation_method": "separate suite access",
            "access_block_log": "EACCES verified",
        },
        "route_provenance": {
            "lexical": {"system": "quanta", "model": "m", "model_revision": "r"},
            "hybrid": {"system": "quanta", "model": "m", "model_revision": "r"},
        },
        "results": [
            {"task_id": "T1", "route": "lexical", "status": "abstained",
             "candidates": [], "timings": {"query_latency_ms": 1.0}, "error": None},
            {"task_id": "T1", "route": "hybrid", "status": "success",
             "candidates": [cand], "timings": {"query_latency_ms": 1.0}, "error": None},
            {"task_id": "T2", "route": "lexical", "status": "abstained",
             "candidates": [], "timings": {"query_latency_ms": 1.0}, "error": None},
            {"task_id": "T2", "route": "hybrid", "status": "abstained",
             "candidates": [], "timings": {"query_latency_ms": 1.0}, "error": None},
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


def test_v2_all_answerable_external_suite_passes_without_invented_no_answer(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v2(tmp_path, answerable_only=True)
    loaded = record_v2(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid")
    assert report["answerable_tasks"] == 2
    assert report["no_gold_tasks"] == 0
    assert report["budgets"]["4000"]["routes"]["hybrid"]["no_gold_abstention"] == ev.NOT_APPLICABLE
    assert report["budgets"]["4000"]["comparison"]["delta"]["no_gold_abstention"] == ev.NOT_APPLICABLE
    assert report["rank_metrics"]["routes"]["hybrid"]["chunk"]["recall_at_5"] == pytest.approx(1.0)


def test_v2_same_candidates_identical_scores_regardless_of_runner(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v2(tmp_path)
    first = ev.evaluate(*record_v2(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    run["runner"]["name"] = "other-runner"
    run["runner"]["run_id"] = "run-other"
    run["route_provenance"]["hybrid"]["model"] = "other-model"
    runner_path2 = tmp_path / "run2.json"
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    runner_path2.write_text(json.dumps(run), encoding="utf-8")
    second = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path2), "lexical", "hybrid")
    assert ev.canonical(first["budgets"]) == ev.canonical(second["budgets"])
    assert ev.canonical(first["rank_metrics"]) == ev.canonical(second["rank_metrics"])


def test_v2_rescore_is_deterministic_under_row_order(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v2(tmp_path)
    first = ev.evaluate(*record_v2(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    run["results"] = list(reversed(run["results"]))
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    second = ev.evaluate(*ev.load_evidence(repo, suite_path, runner_path), "lexical", "hybrid")
    assert ev.canonical(first["budgets"]) == ev.canonical(second["budgets"])
    assert ev.canonical(first["rank_metrics"]) == ev.canonical(second["rank_metrics"])
    assert ev.canonical(first["per_query"]) == ev.canonical(second["per_query"])


def test_v2_partial_span_earns_no_full_cover_credit(tmp_path):
    files = {"a.txt": b"line one\nline two\nline three\n"}
    repo, commit = _write_repo(tmp_path, files)
    file_sha = ev.digest(files["a.txt"])
    gold_sel = b"line one\nline two\n"
    part_sel = b"line one\n"
    q = "find first two lines"
    suite = {
        "schema_version": 2, "suite_id": "partial-v2", "repository_commit": commit,
        "routes": ["lexical", "hybrid"],
        "file_universe": [{"path": "a.txt", "file_sha256": file_sha}],
        "tasks": [
            {"task_id": "T1", "split": "eval", "query": q,
             "query_sha256": ev.digest(q.encode()), "answerable": True,
             "gold": [{"path": "a.txt", "start_line": 1, "end_line": 2,
                       "file_sha256": file_sha, "block_sha256": ev.digest(gold_sel), "grade": 3}]},
            {"task_id": "T2", "split": "eval", "query": "find nothing here",
             "query_sha256": ev.digest(b"find nothing here"), "answerable": False, "gold": []},
        ],
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    part_tokens = len(ev.TOKEN_RE.findall(part_sel.decode()))
    full_tokens = len(ev.TOKEN_RE.findall(gold_sel.decode()))
    part = {"path": "a.txt", "start_line": 1, "end_line": 1, "file_sha256": file_sha,
            "block_sha256": ev.digest(part_sel), "tokens": part_tokens, "rank": 1}
    full = {"path": "a.txt", "start_line": 1, "end_line": 2, "file_sha256": file_sha,
            "block_sha256": ev.digest(gold_sel), "tokens": full_tokens, "rank": 1}
    run = {
        "schema_version": 2, "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "runner": {"name": "r", "revision": "r@1", "run_id": "x", "tokenizer": ev.TOKENIZER,
                   "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION, "gold_access": False,
                   "blinding": "attested", "isolation_method": "attestation only",
                   "access_block_log": "no separate suite access; attested-only"},
        "route_provenance": {
            "lexical": {"system": "s", "model": "m", "model_revision": "r"},
            "hybrid": {"system": "s", "model": "m", "model_revision": "r"},
        },
        "results": [
            {"task_id": "T1", "route": "lexical", "status": "success", "candidates": [part],
             "timings": {"query_latency_ms": 1.0}, "error": None},
            {"task_id": "T1", "route": "hybrid", "status": "success", "candidates": [full],
             "timings": {"query_latency_ms": 1.0}, "error": None},
            {"task_id": "T2", "route": "lexical", "status": "abstained", "candidates": [],
             "timings": {"query_latency_ms": 1.0}, "error": None},
            {"task_id": "T2", "route": "hybrid", "status": "abstained", "candidates": [],
             "timings": {"query_latency_ms": 1.0}, "error": None},
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


def test_v2_typed_failures_score_zero_and_are_counted(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v2(tmp_path)
    run["results"][1] = {
        "task_id": "T1", "route": "hybrid", "status": "timeout", "candidates": [],
        "timings": {"query_latency_ms": 5.0}, "error": {"code": "TIMEOUT", "message": "deadline"},
    }
    report = ev.evaluate(*record_v2(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    hyb = report["rank_metrics"]["routes"]["hybrid"]
    assert hyb["chunk"]["recall_at_10"] == pytest.approx(0.0)
    assert hyb["status_counts"].get("timeout") == 1
    assert report["budgets"]["4000"]["routes"]["hybrid"]["bcy"] == 0
    rows = [r for r in report["per_query"] if r["task_id"] == "T1" and r["route"] == "hybrid"]
    assert rows[0]["error_code"] == "TIMEOUT"
    assert rows[0]["query_latency_ms"] == pytest.approx(5.0)


def test_v2_capped_result_is_scored_but_flagged(tmp_path):
    repo, suite, run, suite_path, runner_path, _ = fixture_v2(tmp_path)
    run["results"][1]["status"] = "capped"
    report = ev.evaluate(*record_v2(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
    assert report["rank_metrics"]["routes"]["hybrid"]["chunk"]["recall_at_5"] == pytest.approx(1.0)
    rows = [r for r in report["per_query"] if r["task_id"] == "T1" and r["route"] == "hybrid"]
    assert rows[0]["status"] == "capped"


def test_v2_blinding_values_preserved(tmp_path):
    for blinding in ("isolated", "attested"):
        repo, suite, run, suite_path, runner_path, _ = fixture_v2(tmp_path / blinding, blinding=blinding)
        report = ev.evaluate(*record_v2(repo, suite, run, suite_path, runner_path), "lexical", "hybrid")
        assert report["blinding"] == blinding
        assert report["runner"]["blinding"] == blinding


def test_v2_confidence_interval_available_on_sufficient_sample(tmp_path):
    files = {"a.txt": b"alpha one\nalpha two\n"}
    repo, commit = _write_repo(tmp_path, files)
    file_sha = ev.digest(files["a.txt"])
    line1 = b"alpha one\n"
    tasks = []
    for i in range(20):
        q = f"query number {i} alpha one"
        tasks.append({
            "task_id": f"T{i:02d}", "split": "eval", "query": q,
            "query_sha256": ev.digest(q.encode()), "answerable": True,
            "gold": [{"path": "a.txt", "start_line": 1, "end_line": 1,
                      "file_sha256": file_sha, "block_sha256": ev.digest(line1), "grade": 2}],
        })
    suite = {
        "schema_version": 2, "suite_id": "ci-v2", "repository_commit": commit,
        "routes": ["lexical", "hybrid"],
        "file_universe": [{"path": "a.txt", "file_sha256": file_sha}],
        "tasks": tasks,
    }
    _, pack, _ = ev.validate_suite(repo, suite)
    tokens = len(ev.TOKEN_RE.findall(line1.decode()))
    miss_sel = b"alpha two\n"
    miss_tokens = len(ev.TOKEN_RE.findall(miss_sel.decode()))

    def hit(rank):
        return {"path": "a.txt", "start_line": 1, "end_line": 1, "file_sha256": file_sha,
                "block_sha256": ev.digest(line1), "tokens": tokens, "rank": rank}

    def miss(rank):
        return {"path": "a.txt", "start_line": 2, "end_line": 2, "file_sha256": file_sha,
                "block_sha256": ev.digest(miss_sel), "tokens": miss_tokens, "rank": rank}

    results = []
    for i in range(20):
        tid = f"T{i:02d}"
        # Lexical always hits; hybrid hits on even tasks only.
        results.append({"task_id": tid, "route": "lexical", "status": "success",
                        "candidates": [hit(1)], "timings": {"query_latency_ms": 1.0}, "error": None})
        if i % 2 == 0:
            results.append({"task_id": tid, "route": "hybrid", "status": "success",
                            "candidates": [hit(1)], "timings": {"query_latency_ms": 1.0}, "error": None})
        else:
            results.append({"task_id": tid, "route": "hybrid", "status": "success",
                            "candidates": [miss(1)], "timings": {"query_latency_ms": 1.0}, "error": None})
    run = {
        "schema_version": 2, "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "runner": {"name": "r", "revision": "r@1", "run_id": "ci", "tokenizer": ev.TOKENIZER,
                   "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION, "gold_access": False,
                   "blinding": "isolated", "isolation_method": "separate suite access",
                   "access_block_log": "verified"},
        "route_provenance": {
            "lexical": {"system": "s", "model": "m", "model_revision": "r"},
            "hybrid": {"system": "s", "model": "m", "model_revision": "r"},
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
    assert ci["method"] == "normal_approx"
    assert ci["lower_95"] <= ci["mean"] <= ci["upper_95"]
    assert report["rank_metrics"]["comparison"]["paired_losses"] == 10


@pytest.mark.parametrize(
    "mutation,match",
    [
        (lambda s, r, f: s.update(repository_commit="0" * 40), "checkout HEAD differs"),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(block_sha256="0" * 64), "block hash mismatch"),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(file_sha256="0" * 64), "file hash mismatch"),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(grade=5), "grade"),
        (lambda s, r, f: s["tasks"][0]["gold"][0].update(grade="high"), "grade"),
        (lambda s, r, f: s["file_universe"][0].update(file_sha256="0" * 64), "file universe"),
        (lambda s, r, f: s["tasks"][1].update(answerable=True), "answerable/gold mismatch"),
        (lambda s, r, f: s["tasks"][1].update(
            query=s["tasks"][0]["query"], query_sha256=s["tasks"][0]["query_sha256"]), "query leakage"),
        (lambda s, r, f: r["results"].pop(), "missing task route evidence"),
        (lambda s, r, f: r["results"].append(dict(r["results"][0])), "unexpected/duplicate task route"),
        (lambda s, r, f: r["results"][1]["candidates"][1].update(rank=1), "rank"),
        (lambda s, r, f: r["results"][1]["candidates"][0].update(tokens=1), "token count mismatch"),
        (lambda s, r, f: r["results"][1]["candidates"][0].update(file_sha256="0" * 64), "file hash mismatch"),
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
        (lambda s, r, f: r.__setitem__("schema_version", 1), "schema version mismatch"),
    ],
)
def test_v2_fails_closed_on_invalid_evidence(tmp_path, mutation, match):
    repo, suite, run, suite_path, runner_path, files = fixture_v2(tmp_path)
    mutation(suite, run, files)
    with pytest.raises(ev.EvidenceError, match=match):
        record_v2(repo, suite, run, suite_path, runner_path)


def test_v2_excluded_but_tracked_candidate_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v2(tmp_path)
    file_sha, block_sha, tokens = _span_meta(files["excluded.txt"], 1, 1)
    run["results"][1]["candidates"][0] = {
        "path": "excluded.txt", "start_line": 1, "end_line": 1,
        "file_sha256": file_sha, "block_sha256": block_sha, "tokens": tokens, "rank": 1,
    }
    # Re-sequence remaining ranks to isolate the universe failure.
    for i, cand in enumerate(run["results"][1]["candidates"], start=1):
        cand["rank"] = i
    with pytest.raises(ev.EvidenceError, match="excluded from file universe"):
        record_v2(repo, suite, run, suite_path, runner_path)


def test_v2_gold_in_excluded_file_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v2(tmp_path)
    file_sha, block_sha, _ = _span_meta(files["excluded.txt"], 1, 1)
    suite["tasks"][0]["gold"][0] = {
        "path": "excluded.txt", "start_line": 1, "end_line": 1,
        "file_sha256": file_sha, "block_sha256": block_sha, "grade": 2,
    }
    with pytest.raises(ev.EvidenceError, match="excluded from file universe"):
        record_v2(repo, suite, run, suite_path, runner_path)


def test_v2_unsafe_candidate_path_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v2(tmp_path)
    run["results"][1]["candidates"][0]["path"] = "../a.txt"
    with pytest.raises(ev.EvidenceError, match="unsafe repository path"):
        record_v2(repo, suite, run, suite_path, runner_path)


def test_v2_duplicate_line_spans_are_retained_and_credited_once(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v2(tmp_path)
    dup = dict(run["results"][1]["candidates"][0])
    dup["rank"] = 2
    run["results"][1]["candidates"][1] = dup
    run["results"][1]["candidates"][2]["rank"] = 3
    loaded = record_v2(repo, suite, run, suite_path, runner_path)
    report = ev.evaluate(*loaded, "lexical", "hybrid")
    row = next(
        row
        for row in report["per_query"]
        if row["task_id"] == "T1" and row["route"] == "hybrid"
    )
    assert row["candidates"] == 3
    assert row["ndcg_at_10"] == pytest.approx(
        (7.0 + 1.0 / math.log2(4)) / (7.0 + 1.0 / math.log2(3))
    )


def test_v2_error_result_with_candidates_rejected(tmp_path):
    repo, suite, run, suite_path, runner_path, files = fixture_v2(tmp_path)
    run["results"][1]["status"] = "error"
    run["results"][1]["error"] = {"code": "E", "message": "m"}
    with pytest.raises(ev.EvidenceError, match="cannot contain candidates"):
        record_v2(repo, suite, run, suite_path, runner_path)


def test_v2_cli_freeze_evaluate_roundtrip(tmp_path, capsys):
    repo, suite, run, suite_path, runner_path, _ = fixture_v2(tmp_path)
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    pack_path = tmp_path / "pack.json"
    report_path = tmp_path / "report.json"
    assert ev.main(["freeze", "--repo", str(repo), "--suite", str(suite_path), "--output", str(pack_path)]) == 0
    assert capsys.readouterr().out.strip() == run["query_pack_sha256"]
    text = pack_path.read_text(encoding="utf-8")
    assert "gold" not in text
    assert "grade" not in text
    runner_path.write_text(json.dumps(run), encoding="utf-8")
    assert ev.main(["evaluate", "--repo", str(repo), "--suite", str(suite_path),
                    "--runner", str(runner_path), "--baseline-route", "lexical",
                    "--candidate-route", "hybrid", "--output", str(report_path)]) == 0
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert report["schema_version"] == 2
    assert report["rank_metrics"]["comparison"]["sample_count"] == 1
