"""Source-oracle suite generation has independent source and route expectations."""

from __future__ import annotations

import copy
import json
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import source_oracle_suite


def _source_repo(
    tmp_path: Path, extra_files: dict[str, bytes] | None = None
) -> tuple[Path, str, dict[str, bytes]]:
    files = {
        "a.go": b"package sample\ntype Param struct{}\n",
        "b.go": b"package sample\n// Param is mentioned here.\nfunc Next() {}\n",
        **(extra_files or {}),
    }
    repo = tmp_path / "source"
    repo.mkdir()
    for name, data in files.items():
        (repo / name).write_bytes(data)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "--", *files], check=True)
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


def test_source_oracle_gold_projects_a_late_crlf_line_from_byte_offsets(tmp_path):
    raw = b"x\n" * 1_000 + b"Needle\r\n"
    repo, commit, files = _source_repo(tmp_path, {"late.txt": raw})
    source = ev.SourceSnapshot(repo, commit)
    oracle = ev.source_oracle.SourceOracleIndex(
        {path: (source.file(path)[0], ev.digest(contents)) for path, contents in files.items()},
        {"Needle"},
    )
    for _ in range(2):
        assert ev.source_oracle_gold(
            source, oracle, ev.source_oracle.ASCII_IDENTIFIER_WORD, "Needle"
        ) == [
            {
                "path": "late.txt",
                "start_byte": 2_000,
                "end_byte": 2_008,
                "start_line": 1_001,
                "end_line": 1_001,
                "file_sha256": ev.digest(raw),
                "block_sha256": ev.digest(b"Needle\r\n"),
                "grade": 3,
            }
        ]


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


def test_go_indexed_local_name_includes_interface_methods_and_aliases(tmp_path):
    raw = (
        b"package sample\n"
        b"type Reader interface {\n"
        b"\tPush()\n"
        b"}\n"
        b"type Alias = Reader\n"
        b"type Writer struct{}\n"
        b"func (w *Writer) Push() {}\n"
        b"type AliasInterface = interface { Flush() }\n"
        b"var dynamic interface { Hidden() }\n"
    )
    repo, commit, files = _source_repo(tmp_path, {"symbols.go": raw})
    baseline = _baseline(commit, files)
    lines = raw.splitlines(keepends=True)
    for task_id, query, line in (
        ("T3", "Push", 7),
        ("T4", "Alias", 5),
        ("T5", "Hidden", 9),
        ("T6", "Flush", 8),
    ):
        task = copy.deepcopy(baseline["tasks"][0])
        start = sum(len(entry) for entry in lines[: line - 1])
        task.update(
            task_id=task_id,
            query=query,
            query_sha256=ev.digest(query.encode()),
            query_family_id=task_id,
            gold=[
                {
                    "path": "symbols.go",
                    "start_byte": start,
                    "end_byte": start + len(lines[line - 1]),
                    "start_line": line,
                    "end_line": line,
                    "file_sha256": ev.digest(raw),
                    "block_sha256": ev.digest(lines[line - 1]),
                    "grade": 1,
                }
            ],
        )
        baseline["tasks"].append(task)
    source = ev.SourceSnapshot(repo, commit)
    oracle = ev.source_oracle.SourceOracleIndex(
        {path: (source.file(path)[0], ev.digest(contents)) for path, contents in files.items()},
        {"Push", "Alias", "Hidden", "Flush"},
    )

    def declaration(name: bytes, occurrence: int = 0) -> dict:
        start = raw.index(name, raw.index(name) + 1) if occurrence else raw.index(name)
        return {
            "path": "symbols.go",
            "file_sha256": ev.digest(raw),
            "start_byte": start,
            "end_byte": start + len(name),
            "grade": 3,
        }

    assert oracle.expected_rows(ev.source_oracle.GO_EXACT_LOCAL_NAME, "Push", "symbol") == [
        declaration(b"Push"),
        declaration(b"Push", 1),
    ]
    assert oracle.expected_rows(ev.source_oracle.GO_EXACT_LOCAL_NAME, "Alias", "symbol") == [
        declaration(b"Alias")
    ]
    assert oracle.expected_rows(ev.source_oracle.GO_EXACT_LOCAL_NAME, "Flush", "symbol") == [
        declaration(b"Flush")
    ]
    assert oracle.expected_rows(ev.source_oracle.GO_EXACT_LOCAL_NAME, "Hidden", "symbol") == []

    derived = source_oracle_suite.derive_suites(repo, baseline)
    for mode in ("go-declaration-file", "go-declaration-symbol"):
        suite = derived[mode][0]
        tasks = {task["query"]: task for task in suite["tasks"]}
        assert tasks["Push"]["source_oracle"]["contract"] == ev.source_oracle.GO_EXACT_LOCAL_NAME
        assert tasks["Push"]["gold"][0]["start_line"] == 3
        assert tasks["Alias"]["gold"][0]["start_line"] == 5
        assert tasks["Flush"]["gold"][0]["start_line"] == 8
        assert tasks["Hidden"]["answerable"] is False
        assert tasks["Hidden"]["gold"] == []
        if mode.endswith("symbol"):
            assert tasks["Push"]["declaration_judgments"] == [
                declaration(b"Push"),
                declaration(b"Push", 1),
            ]
            assert tasks["Alias"]["declaration_judgments"] == [declaration(b"Alias")]
            assert tasks["Flush"]["declaration_judgments"] == [declaration(b"Flush")]
            assert tasks["Hidden"]["declaration_judgments"] == []
        else:
            assert tasks["Push"]["file_judgments"] == [
                {"path": "symbols.go", "file_sha256": ev.digest(raw), "grade": 3}
            ]
        legacy = copy.deepcopy(suite)
        next(task for task in legacy["tasks"] if task["query"] == "Push")["source_oracle"]["contract"] = (
            "go_exact_local_name_v1"
        )
        with pytest.raises(ev.EvidenceError, match="unsupported source oracle contract"):
            ev.validate_suite(repo, legacy)


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
    assert len(manifest["artifacts"]) == 18
    for artifact in manifest["artifacts"]:
        assert ev.digest((output / artifact["path"]).read_bytes()) == artifact["sha256"]
    for dependency in (
        "tools/ci/lint/handoff_validation.py",
        "tools/ci/proof_json.py",
    ):
        source = Path(__file__).resolve().parents[3] / dependency
        copied = output / "tool-sources" / dependency
        assert copied.read_bytes() == source.read_bytes()
        assert {"path": dependency, "sha256": ev.digest(source.read_bytes())} in manifest[
            "tool_files"
        ]
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


def test_source_oracle_builder_refuses_claims_and_replaces_baseline_gold(tmp_path, monkeypatch):
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
    for suite, _pack in source_oracle_suite.derive_suites(repo, bad).values():
        derived = suite["tasks"][0]
        assert derived["answerable"] is False
        assert derived["gold"] == []

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


def test_source_oracle_builder_rebinds_answerability_and_gold_per_mode(tmp_path):
    repo, commit, files = _source_repo(tmp_path)
    baseline = _baseline(commit, files)
    task = baseline["tasks"][0]
    raw = files["b.go"]
    lines = raw.splitlines(keepends=True)
    start = len(lines[0])
    task["query"] = "mentioned"
    task["query_sha256"] = ev.digest(b"mentioned")
    task["gold"] = [
        {
            "path": "b.go",
            "start_byte": start,
            "end_byte": start + len(lines[1]),
            "start_line": 2,
            "end_line": 2,
            "file_sha256": ev.digest(raw),
            "block_sha256": ev.digest(lines[1]),
            "grade": 1,
        }
    ]
    baseline["tasks"] = [task]
    ev.validate_suite(repo, baseline)
    suites = source_oracle_suite.derive_suites(repo, baseline)
    word_task = suites["identifier-word-file"][0]["tasks"][0]
    assert word_task["answerable"] is True
    assert word_task["gold"][0]["path"] == "b.go"
    assert word_task["gold"][0]["start_line"] == 2
    assert word_task["gold"][0]["grade"] == 3
    for mode in ("go-declaration-file", "go-declaration-symbol"):
        derived = suites[mode][0]["tasks"][0]
        assert derived["answerable"] is False
        assert derived["gold"] == []
        assert derived["file_judgments" if mode.endswith("file") else "declaration_judgments"] == []


def test_source_oracle_validator_rejects_a_later_matching_gold_line(tmp_path):
    raw = b"package sample\ntype Param struct{}\n// Param repeated here.\n"
    repo, commit, files = _source_repo(tmp_path, {"a.go": raw})
    baseline = _baseline(commit, files)
    suite = source_oracle_suite.derive_suites(repo, baseline)["identifier-word-file"][0]
    assert suite["tasks"][0]["gold"][0]["start_line"] == 2

    later_line = raw.splitlines(keepends=True)[2]
    changed = copy.deepcopy(suite)
    changed["tasks"][0]["gold"] = [
        {
            "path": "a.go",
            "start_byte": len(raw) - len(later_line),
            "end_byte": len(raw),
            "start_line": 3,
            "end_line": 3,
            "file_sha256": ev.digest(raw),
            "block_sha256": ev.digest(later_line),
            "grade": 3,
        }
    ]
    with pytest.raises(ev.EvidenceError, match="gold differs from canonical first match"):
        ev.validate_suite(repo, changed)


def test_source_oracle_builder_uses_evaluator_line_projection_for_cr_only_source(tmp_path):
    repo, commit, files = _source_repo(tmp_path, {"c.txt": b"header\rTarget\r"})
    baseline = _baseline(commit, files)
    task = baseline["tasks"][0]
    raw = files["c.txt"]
    lines = raw.splitlines(keepends=True)
    start = len(lines[0])
    task["query"] = "Target"
    task["query_sha256"] = ev.digest(b"Target")
    task["gold"] = [
        {
            "path": "c.txt",
            "start_byte": start,
            "end_byte": start + len(lines[1]),
            "start_line": 2,
            "end_line": 2,
            "file_sha256": ev.digest(raw),
            "block_sha256": ev.digest(lines[1]),
            "grade": 1,
        }
    ]
    ev.validate_suite(repo, baseline)
    suites = source_oracle_suite.derive_suites(repo, baseline)
    word_task = suites["identifier-word-file"][0]["tasks"][0]
    assert word_task["gold"] == [dict(task["gold"][0], grade=3)]
    for mode in ("go-declaration-file", "go-declaration-symbol"):
        assert suites[mode][0]["tasks"][0]["gold"] == []


def test_source_oracle_rejects_oversized_source_before_unbounded_read(tmp_path, monkeypatch):
    repo, commit, files = _source_repo(tmp_path)
    baseline = _baseline(commit, files)
    suite = source_oracle_suite.derive_suites(repo, baseline)["identifier-word-file"][0]
    original_read_bytes = Path.read_bytes

    def refuse_unbounded_source_read(path: Path) -> bytes:
        if path.is_relative_to(repo):
            raise AssertionError("source oracle used an unbounded source read")
        return original_read_bytes(path)

    monkeypatch.setattr(Path, "read_bytes", refuse_unbounded_source_read)
    max_files = ev.source_oracle.MAX_FILES
    max_queries = ev.source_oracle.MAX_QUERIES
    monkeypatch.setattr(ev.source_oracle, "MAX_FILES", 1)
    with pytest.raises(ev.EvidenceError, match="source oracle file limit exceeded"):
        source_oracle_suite.derive_suites(repo, baseline)
    monkeypatch.setattr(ev.source_oracle, "MAX_FILES", max_files)
    monkeypatch.setattr(ev.source_oracle, "MAX_QUERIES", 1)
    with pytest.raises(ev.EvidenceError, match="source oracle query limit exceeded"):
        source_oracle_suite.derive_suites(repo, baseline)
    monkeypatch.setattr(ev.source_oracle, "MAX_QUERIES", max_queries)
    monkeypatch.setattr(ev.source_oracle, "MAX_SOURCE_BYTES", 8)
    with pytest.raises(ev.EvidenceError, match="source oracle byte limit exceeded"):
        source_oracle_suite.derive_suites(repo, baseline)
    with pytest.raises(ev.EvidenceError, match="source oracle byte limit exceeded"):
        ev.validate_suite(repo, suite)
