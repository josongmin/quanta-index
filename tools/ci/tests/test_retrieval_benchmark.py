"""Real Git source, blinded runner boundary and budgeted route scoring."""

from __future__ import annotations

import json
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
