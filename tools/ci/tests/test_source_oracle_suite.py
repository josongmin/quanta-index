"""Source-oracle suite generation has independent source and route expectations."""

from __future__ import annotations

import copy
import json
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import source_oracle_suite


def _source_repo(tmp_path: Path) -> tuple[Path, str, dict[str, bytes]]:
    files = {
        "a.go": b"package sample\ntype Param struct{}\n",
        "b.go": b"package sample\n// Param is mentioned here.\nfunc Next() {}\n",
    }
    repo = tmp_path / "source"
    repo.mkdir()
    for name, data in files.items():
        (repo / name).write_bytes(data)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "a.go", "b.go"], check=True)
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
    commit = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True
    ).strip()
    return repo, commit, files


def _baseline(commit: str, files: dict[str, bytes]) -> dict:
    universe = [
        {"path": path, "file_sha256": ev.digest(raw)} for path, raw in sorted(files.items())
    ]

    def task(task_id: str, query: str, path: str, line: int) -> dict:
        raw = files[path]
        lines = raw.splitlines(keepends=True)
        start = sum(len(entry) for entry in lines[: line - 1])
        end = start + len(lines[line - 1])
        return {
            "task_id": task_id,
            "query": query,
            "query_sha256": ev.digest(query.encode()),
            "query_family_id": task_id,
            "split": "eval",
            "answerable": True,
            "gold": [
                {
                    "path": path,
                    "start_byte": start,
                    "end_byte": end,
                    "start_line": line,
                    "end_line": line,
                    "file_sha256": ev.digest(raw),
                    "block_sha256": ev.digest(lines[line - 1]),
                    "grade": 1,
                }
            ],
        }

    return {
        "schema_version": 3,
        "suite_id": "source-oracle-fixture",
        "repository_commit": commit,
        "comparison_contract": {
            "top_k": 10,
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "output_unit_policy": "rank_prefix",
            "span_unit": ev.SPAN_UNIT,
        },
        "routes": ["lexical", "legacy-second-route"],
        "file_universe": universe,
        "file_universe_digest": ev.universe_digest(universe),
        "tasks": [task("T1", "Param", "a.go", 2), task("T2", "Next", "b.go", 3)],
    }


def test_source_oracle_builder_emits_single_route_blind_suites_and_bound_manifest(tmp_path):
    repo, commit, files = _source_repo(tmp_path)
    baseline = _baseline(commit, files)
    baseline_path = tmp_path / "baseline.json"
    baseline_path.write_text(json.dumps(baseline), encoding="utf-8")
    output = tmp_path / "new-output"
    manifest = source_oracle_suite.write_suites(repo, baseline_path, output)
    assert manifest["qualification"] == "diagnostic_unqualified"
    assert manifest["repository_commit"] == commit
    assert manifest["input_suite_sha256"] == ev.digest(baseline_path.read_bytes())
    assert len(manifest["artifacts"]) == 10
    for artifact in manifest["artifacts"]:
        assert ev.digest((output / artifact["path"]).read_bytes()) == artifact["sha256"]
    for mode, route in (
        ("identifier-word-file", "lexical"),
        ("go-declaration-file", "lexical"),
        ("go-declaration-symbol", "symbol"),
    ):
        suite = json.loads((output / f"{mode}-suite.json").read_bytes())
        pack = json.loads((output / f"{mode}-blind-pack.json").read_bytes())
        assert suite["routes"] == [route]
        assert pack["routes"] == [route]
        assert "source_oracle" not in ev.canonical(pack).decode()
        assert ev.validate_suite(repo, suite)[1] == pack
    word = json.loads((output / "identifier-word-file-suite.json").read_bytes())
    declaration = json.loads((output / "go-declaration-file-suite.json").read_bytes())
    symbol = json.loads((output / "go-declaration-symbol-suite.json").read_bytes())
    assert [row["path"] for row in word["tasks"][0]["file_judgments"]] == ["a.go", "b.go"]
    assert [row["path"] for row in declaration["tasks"][0]["file_judgments"]] == ["a.go"]
    name = files["a.go"].index(b"Param")
    assert [
        (row["path"], row["start_byte"], row["end_byte"])
        for row in symbol["tasks"][0]["declaration_judgments"]
    ] == [("a.go", name, name + 5)]
    for mode, candidate_unit in (
        ("identifier-word-file", "distinct_file"),
        ("go-declaration-symbol", "symbol"),
    ):
        suite = json.loads((output / f"{mode}-suite.json").read_bytes())
        pack = json.loads((output / f"{mode}-blind-pack.json").read_bytes())
        route = suite["routes"][0]
        results = []
        for task in suite["tasks"]:
            match_path = "a.go" if task["task_id"] == "T1" else "b.go"
            candidate = {"path": match_path, "rank": 1}
            if candidate_unit == "symbol":
                label = task["declaration_judgments"][0]
                candidate["span_accounting"] = {
                    "unit_kind": "symbol",
                    "unit_id": task["task_id"],
                    "indexed_start_byte": label["start_byte"],
                    "indexed_end_byte": label["end_byte"],
                }
            results.append(
                {
                    "task_id": task["task_id"],
                    "route": route,
                    "status": "success",
                    "rank_unit": candidate_unit,
                    "candidates": [candidate],
                }
            )
        run = {
            "comparison_contract": suite["comparison_contract"],
            "route_provenance": {route: {"capture_id": "q0"}},
            "captures": {"q0": {"system": "quanta"}},
            "runner": {"name": "fixture"},
            "span_accounting_version": 1,
            "results": results,
        }
        report = ev.evaluate_diagnostic(suite, pack, run)
        assert report["status"] == "diagnostic_unqualified"
        assert report["route"] == route
        kind = "declaration_judgments" if candidate_unit == "symbol" else "file_judgments"
        assert report["judgment_metrics"][kind]["routes"][route]["eligible_task_ids"] == [
            "T1",
            "T2",
        ]
    with pytest.raises(ev.EvidenceError, match="already exists"):
        source_oracle_suite.write_suites(repo, baseline_path, output)
    with pytest.raises(ev.EvidenceError, match="outside source and tool checkouts"):
        source_oracle_suite.write_suites(repo, baseline_path, repo / "forbidden-output")
    assert not (repo / "forbidden-output").exists()


def test_source_oracle_builder_refuses_claims_and_source_gold_conflicts(tmp_path, monkeypatch):
    repo, commit, files = _source_repo(tmp_path)
    baseline = _baseline(commit, files)
    baseline["tasks"][0]["label_review"] = {"assessment": "unreviewed"}
    baseline["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    with pytest.raises(ev.EvidenceError, match="unannotated task"):
        source_oracle_suite.derive_suites(repo, baseline)
    bad = copy.deepcopy(baseline)
    bad["tasks"][0].pop("label_review")
    bad["tasks"][0]["query"] = "ParamExtra"
    bad["tasks"][0]["query_sha256"] = ev.digest(b"ParamExtra")
    with pytest.raises(ev.EvidenceError, match="lacks a positive judgment"):
        source_oracle_suite.derive_suites(repo, bad)

    baseline_path = tmp_path / "baseline.json"
    baseline_path.write_text(json.dumps(_baseline(commit, files)), encoding="utf-8")
    real_digests = source_oracle_suite._tool_digests
    calls = 0

    def drifting_digests():
        nonlocal calls
        calls += 1
        rows = real_digests()
        if calls > 1:
            rows[0]["sha256"] = "0" * 64
        return rows

    monkeypatch.setattr(source_oracle_suite, "_tool_digests", drifting_digests)
    output = tmp_path / "blocked-output"
    with pytest.raises(ev.EvidenceError, match="tool source changed"):
        source_oracle_suite.write_suites(repo, baseline_path, output)
    assert not output.exists()
