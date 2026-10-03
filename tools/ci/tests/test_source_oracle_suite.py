"""Source-oracle suite generation has independent source and route expectations."""

from __future__ import annotations

import copy
import json
import subprocess
from pathlib import Path

import jsonschema
import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import source_oracle_suite


@pytest.mark.parametrize("suffix", ["ts", "tsx"])
def test_typescript_oracles_use_the_producer_compatibility_grammar(suffix):
    from tools.benchmark.retrieval import gold_oracle, source_oracle

    raw = (
        b'export type * from "./other";\n'
        b'export type * as Other from "./other";\n'
        b"export function Locate() {\n"
        b"  return runnerImport<typeof import('./basic')>(fixture('cjs.js'),);\n"
        + (b"  return <div />;\n" if suffix == "tsx" else b"")
        + b"}\n"
    )
    rows = source_oracle.declaration_census("typescript", "input." + suffix, raw)
    assert [raw[start:end] for start, end, *_ in rows] == [b"Locate"]
    spans, refusal = gold_oracle._definition_spans(
        raw, b"Locate", "typescript", path="input." + suffix
    )
    assert refusal is None
    assert len(spans) == 1
    assert raw[spans[0][0] : spans[0][1]] == b"Locate"
    with pytest.raises(source_oracle.SourceOracleError, match="parse error"):
        source_oracle.declaration_census(
            "typescript", "broken." + suffix, raw + b"function Broken( {"
        )


def test_vendored_parser_cache_refuses_identity_and_binary_tampering(tmp_path):
    from hashlib import sha256

    from tools.benchmark.retrieval import declaration_parsers

    binary = tmp_path / "parser.so"
    binary.write_bytes(b"fixed parser bytes")
    marker = tmp_path / "ready.json"
    identity = {"grammar": "typescript"}
    marker.write_text(
        json.dumps({"identity": identity, "binary_sha256": sha256(binary.read_bytes()).hexdigest()})
    )
    assert declaration_parsers._checked_library(tmp_path, identity) == binary
    with pytest.raises(ValueError, match="identity differs"):
        declaration_parsers._checked_library(tmp_path, {"grammar": "tsx"})
    binary.write_bytes(b"changed parser bytes")
    with pytest.raises(ValueError, match="binary differs"):
        declaration_parsers._checked_library(tmp_path, identity)
    marker.write_text("null")
    with pytest.raises(ValueError, match="identity differs"):
        declaration_parsers._checked_library(tmp_path, identity)


def test_vendored_parser_has_no_unpatched_fallback_when_compiler_is_missing(tmp_path, monkeypatch):
    from tools.benchmark.retrieval import declaration_parsers

    monkeypatch.setenv("QUANTA_CENSUS_PARSER_CACHE", str(tmp_path / "empty-parser-cache"))
    monkeypatch.setattr(declaration_parsers.shutil, "which", lambda _name: None)
    with pytest.raises(ValueError, match="C compiler unavailable"):
        declaration_parsers._library("typescript")


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

    def declaration(fragment: bytes) -> dict:
        start = raw.index(fragment)
        return {
            "path": "symbols.go",
            "file_sha256": ev.digest(raw),
            "start_byte": start,
            "end_byte": start + len(fragment),
            "grade": 3,
        }

    assert oracle.expected_rows(ev.source_oracle.GO_EXACT_LOCAL_NAME, "Push", "symbol") == [
        declaration(b"Push()"),
        declaration(b"func (w *Writer) Push() {}"),
    ]
    assert oracle.expected_rows(ev.source_oracle.GO_EXACT_LOCAL_NAME, "Alias", "symbol") == [
        declaration(b"Alias = Reader")
    ]
    assert oracle.expected_rows(ev.source_oracle.GO_EXACT_LOCAL_NAME, "Flush", "symbol") == [
        declaration(b"Flush()")
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
                declaration(b"Push()"),
                declaration(b"func (w *Writer) Push() {}"),
            ]
            assert tasks["Alias"]["declaration_judgments"] == [declaration(b"Alias = Reader")]
            assert tasks["Flush"]["declaration_judgments"] == [declaration(b"Flush()")]
            assert tasks["Hidden"]["declaration_judgments"] == []
        else:
            assert tasks["Push"]["file_judgments"] == [
                {"path": "symbols.go", "file_sha256": ev.digest(raw), "grade": 3}
            ]
        for old_contract in ("go_exact_local_name_v1", "go_exact_local_name_v2"):
            legacy = copy.deepcopy(suite)
            next(task for task in legacy["tasks"] if task["query"] == "Push")["source_oracle"][
                "contract"
            ] = old_contract
            with pytest.raises(ev.EvidenceError, match="unsupported source oracle contract"):
                ev.validate_suite(repo, legacy)


def test_go_source_oracle_symbol_judgment_matches_definition_not_shared_context(tmp_path):
    raw = b"package sample\nfunc First() {}; func Second() {}\n"
    repo, commit, files = _source_repo(tmp_path, {"same_line.go": raw})
    source = ev.SourceSnapshot(repo, commit)
    oracle = ev.source_oracle.SourceOracleIndex(
        {path: (source.file(path)[0], ev.digest(contents)) for path, contents in files.items()},
        {"First"},
    )
    judgments = oracle.expected_rows(ev.source_oracle.GO_EXACT_LOCAL_NAME, "First", "symbol")
    first_start = raw.index(b"func First() {}")
    second_start = raw.index(b"func Second() {}")
    assert [(row["start_byte"], row["end_byte"]) for row in judgments] == [
        (first_start, first_start + len(b"func First() {}"))
    ]

    def candidate(start: int, end: int, unit_id: str) -> dict:
        return {
            "path": "same_line.go",
            "start_byte": raw.index(b"func First() {}"),
            "end_byte": len(raw),
            "span_accounting": {
                "unit_kind": "symbol",
                "unit_id": unit_id,
                "indexed_start_byte": start,
                "indexed_end_byte": end,
            },
        }

    wrong = candidate(second_start, second_start + len(b"func Second() {}"), "second")
    right = candidate(first_start, first_start + len(b"func First() {}"), "first")
    assert ev.declaration_recall_at_k([wrong], judgments, 10) == 0.0
    assert ev.declaration_mrr_at_k([wrong, right], judgments, 10) == 0.5


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
    definition = files["a.go"].index(b"Param struct{}")
    assert [
        (row["path"], row["start_byte"], row["end_byte"])
        for row in symbol["tasks"][0]["declaration_judgments"]
    ] == [("a.go", definition, definition + len(b"Param struct{}"))]
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


VARIANT_GO = (
    b"package sample\n"
    b"type Binder interface {\n"
    b"\tBindBody()\n"
    b"}\n"
    b"func BindJSON() {}\n"
    b"func BindXML() {}\n"
    b"func bindQuery() {}\n"
    b"type HTMLRender struct{}\n"
    b"func (HTMLRender) Render() {}\n"
    b"func render_html_page() {}\n"
    b"func ReadJSON() {}\n"
    b"// BindYAML is only mentioned in a comment.\n"
)


def _variant_oracle(
    tmp_path, extra: dict[str, bytes] | None = None, words: frozenset = frozenset({"Param"})
):
    tmp_path.mkdir(parents=True, exist_ok=True)
    repo, commit, files = _source_repo(
        tmp_path,
        {
            "variant.go": VARIANT_GO,
            "variant_test.go": b"package sample\nfunc BindJSONTest() {}\n",
            **(extra or {}),
        },
    )
    oracle = ev.source_oracle.SourceOracleIndex(
        {path: (raw, ev.digest(raw)) for path, raw in files.items()}, set(words)
    )
    return oracle


def test_go_name_prefix_and_infix_are_case_sensitive_and_exhaustive(tmp_path):
    so = ev.source_oracle
    oracle = _variant_oracle(tmp_path)
    assert oracle.matched_names(so.GO_NAME_PREFIX, "Bind") == [
        "BindBody",
        "BindJSON",
        "BindJSONTest",
        "BindXML",
        "Binder",
    ]
    assert oracle.matched_names(so.GO_NAME_PREFIX, "bin") == ["bindQuery"]
    assert [
        row["path"] for row in oracle.expected_rows(so.GO_NAME_PREFIX, "BindJSON", "distinct_file")
    ] == [
        "variant.go",
        "variant_test.go",
    ]
    # Infix counts every declaration containing the fragment, including prefixes.
    assert oracle.matched_names(so.GO_NAME_INFIX, "JSO") == ["BindJSON", "BindJSONTest", "ReadJSON"]
    assert oracle.matched_names(so.GO_NAME_INFIX, "YAML") == []
    for contract in (so.GO_NAME_PREFIX, so.GO_NAME_INFIX):
        with pytest.raises(so.SourceOracleError, match="three characters"):
            oracle.expected_rows(contract, "Bi", "distinct_file")
        with pytest.raises(so.SourceOracleError, match="query form"):
            oracle.expected_rows(contract, "Bindé", "distinct_file")


def test_go_name_components_split_only_existing_boundaries_contiguously(tmp_path):
    so = ev.source_oracle
    assert so.name_components("HTMLRender") == ("html", "render")
    assert so.name_components("render_html_page") == ("render", "html", "page")
    assert so.name_components("toHTTP2Server") == ("to", "http", "2", "server")
    oracle = _variant_oracle(tmp_path)
    assert oracle.matched_names(so.GO_NAME_COMPONENTS, "html render") == ["HTMLRender"]
    assert oracle.matched_names(so.GO_NAME_COMPONENTS, "render html") == ["render_html_page"]
    assert oracle.matched_names(so.GO_NAME_COMPONENTS, "bind json") == ["BindJSON", "BindJSONTest"]
    # Non-contiguous components and invented synonyms do not match.
    assert oracle.matched_names(so.GO_NAME_COMPONENTS, "render page") == []
    with pytest.raises(so.SourceOracleError, match="query form"):
        oracle.expected_rows(so.GO_NAME_COMPONENTS, "HTMLRender", "distinct_file")


def test_go_name_osa1_separates_unique_ambiguous_and_exact_collision(tmp_path):
    so = ev.source_oracle
    for first, second, expected in (
        ("BindJSON", "BindJSN", True),
        ("BindJSON", "BindJSONx", True),
        ("BindJSON", "BindJSOM", True),
        ("BindJSON", "BindJOSN", True),
        ("BindJSON", "BindJONS", False),
        ("BindJSON", "BidnJSNO", False),
    ):
        assert so.osa_distance_at_most_one(first, second) is expected
    oracle = _variant_oracle(tmp_path)
    assert oracle.matched_names(so.GO_NAME_OSA1, "BindJOSN") == ["BindJSON"]
    assert oracle.matched_names(so.GO_NAME_OSA1, "BindXQQ") == []
    assert oracle.matched_names(so.GO_NAME_OSA1, "ReadJSO") == ["ReadJSON"]
    assert oracle.matched_names(so.GO_NAME_OSA1, "BindJSOX") == ["BindJSON"]
    assert oracle.matched_names(so.GO_NAME_OSA1, "BindXMLJ") == ["BindXML"]
    ambiguous = _variant_oracle(
        tmp_path / "ambiguous", {"more.go": b"package sample\nfunc BindXMM() {}\n"}
    )
    assert ambiguous.matched_names(so.GO_NAME_OSA1, "BindXMX") == ["BindXML", "BindXMM"]
    # An edited query that is itself a declaration is an exact collision, not its own typo.
    assert "BindXML" not in ambiguous.matched_names(so.GO_NAME_OSA1, "BindXML")
    assert ambiguous.expected_rows(so.GO_EXACT_LOCAL_NAME, "BindXMM", "distinct_file")

    # The product typo request folds ASCII case. Preserve the historical
    # case-sensitive oracle for old captures and bind new labels separately.
    folded = _variant_oracle(
        tmp_path / "folded",
        {"case.go": b"package sample\nfunc DELETE() {}\nfunc deleteX() {}\n"},
    )
    assert folded.matched_names(so.GO_NAME_OSA1, "dlete") == []
    assert folded.matched_names(so.GO_NAME_OSA1_CASEFOLD, "dlete") == ["DELETE"]
    assert folded.matched_names(so.GO_NAME_OSA1_CASEFOLD, "delete") == ["deleteX"]
    for invalid in ("ab", "2lete", "d" * 65):
        with pytest.raises(so.SourceOracleError, match="product-admissible"):
            folded.expected_rows(so.GO_NAME_OSA1_CASEFOLD, invalid, "distinct_file")


def test_go_no_answer_requires_a_complete_parse(tmp_path):
    so = ev.source_oracle
    oracle = _variant_oracle(tmp_path, words=frozenset({"BindYAML"}))
    assert oracle.expected_rows(so.GO_EXACT_LOCAL_NAME, "BindYAML", "distinct_file") == []
    assert [
        r["path"]
        for r in oracle.expected_rows(so.ASCII_IDENTIFIER_WORD, "BindYAML", "distinct_file")
    ] == ["variant.go"]
    broken = _variant_oracle(tmp_path / "broken", {"broken.go": b"package sample\nfunc (\n"})
    with pytest.raises(so.SourceOracleError, match="parse error"):
        broken.expected_rows(so.GO_EXACT_LOCAL_NAME, "BindYAML", "distinct_file")
    with pytest.raises(so.SourceOracleError, match="parse error"):
        broken.expected_rows(so.GO_NAME_PREFIX, "Bin", "distinct_file")


def test_code_search_absence_rejects_content_and_path_matches(tmp_path):
    so = ev.source_oracle
    files = {
        "present.go": (
            b"package sample\n// nearMiss\n",
            ev.digest(b"package sample\n// nearMiss\n"),
        ),
        "nearMiss/file.go": (b"package sample\n", ev.digest(b"package sample\n")),
    }
    oracle = so.SourceOracleIndex(files, {"nearMiss", "absentName"})
    with pytest.raises(so.SourceOracleError, match="content absent contract found a match"):
        oracle.expected_rows(so.ASCII_CODE_SEARCH_ABSENT_CASEFOLD, "NEARMISS", "distinct_file")
    path_only = so.SourceOracleIndex({"nearMiss/file.go": files["nearMiss/file.go"]}, {"nearMiss"})
    with pytest.raises(so.SourceOracleError, match="path match"):
        path_only.expected_rows(so.ASCII_CODE_SEARCH_ABSENT_CASEFOLD, "NEARMISS", "distinct_file")
    assert (
        oracle.expected_rows(so.ASCII_CODE_SEARCH_ABSENT_CASEFOLD, "absentName", "distinct_file")
        == []
    )


def test_default_code_search_absence_rejects_one_edit_fallback():
    so = ev.source_oracle
    raw = b"def test_init(): pass\n"
    oracle = so.SourceOracleIndex({"tests.py": (raw, ev.digest(raw))}, {"test_unit", "Absent"})
    assert (
        oracle.expected_rows(so.ASCII_CODE_SEARCH_ABSENT_CASEFOLD, "test_unit", "distinct_file")
        == []
    )
    with pytest.raises(so.SourceOracleError, match="identifier osa1 absent"):
        oracle.expected_rows(
            so.ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD, "test_unit", "distinct_file"
        )
    assert (
        oracle.expected_rows(
            so.ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD, "Absent", "distinct_file"
        )
        == []
    )


@pytest.mark.parametrize(
    "source_token",
    ["load_json", "load_jsom", "load_jsonx", "load_jso", "load_jsno", "LOAD_JSON"],
)
def test_identifier_osa1_absence_rejects_all_single_edits(source_token):
    so = ev.source_oracle
    raw = f"// {source_token}\n".encode()
    oracle = so.SourceOracleIndex({"source.go": (raw, ev.digest(raw))}, {"load_json"})
    with pytest.raises(so.SourceOracleError, match="found a source token"):
        oracle.expected_rows(so.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD, "load_json", "distinct_file")


def test_identifier_osa1_absence_is_content_only_and_rejects_invalid_unit():
    so = ev.source_oracle
    raw = b"// load_jzzn cafe\n"
    oracle = so.SourceOracleIndex({"load_json/source.go": (raw, ev.digest(raw))}, {"load_json"})
    assert (
        oracle.expected_rows(so.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD, "load_json", "distinct_file")
        == []
    )
    assert oracle.first_match(so.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD, "load_json") is None
    with pytest.raises(so.SourceOracleError, match="unsupported"):
        oracle.expected_rows(so.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD, "load_json", "symbol")
    for invalid in ("ab", "a" * 65, "load-json"):
        with pytest.raises(so.SourceOracleError):
            oracle.expected_rows(so.ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD, invalid, "distinct_file")


def _robustness_baseline(tmp_path):
    repo = tmp_path / "robust"
    repo.mkdir()
    (repo / "variant.go").write_bytes(VARIANT_GO)
    git = ["git", "-C", str(repo)]
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run([*git, "add", "variant.go"], check=True)
    subprocess.run(
        [
            *git,
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "frozen",
        ],
        check=True,
    )
    commit = subprocess.check_output([*git, "rev-parse", "HEAD"], text=True).strip()
    # Reuse the contract envelope only; universe and tasks are replaced below.
    baseline = _baseline(commit, {"a.go": b"x\ny\n", "b.go": b"x\ny\nz\n"})
    universe = [{"path": "variant.go", "file_sha256": ev.digest(VARIANT_GO)}]
    baseline.update(
        file_universe=universe, file_universe_digest=ev.universe_digest(universe), tasks=[]
    )
    lines = VARIANT_GO.splitlines(keepends=True)
    for index, name in enumerate(
        ["BindJSON", "BindXML", "HTMLRender", "ReadJSON", "Render", "bindQuery"], 1
    ):
        line = next(
            i
            for i, text in enumerate(lines, 1)
            if name.encode() + b"(" in text or b"type " + name.encode() + b" " in text
        )
        start = sum(len(entry) for entry in lines[: line - 1])
        baseline["tasks"].append(
            {
                "task_id": f"T{index}",
                "query": name,
                "query_sha256": ev.digest(name.encode()),
                "query_family_id": f"fam-{name}",
                "split": "eval",
                "answerable": True,
                "gold": [
                    {
                        "path": "variant.go",
                        "start_byte": start,
                        "end_byte": start + len(lines[line - 1]),
                        "start_line": line,
                        "end_line": line,
                        "file_sha256": ev.digest(VARIANT_GO),
                        "block_sha256": ev.digest(lines[line - 1]),
                        "grade": 1,
                    }
                ],
            }
        )
    return repo, baseline


def test_typo_gold_partition_separates_intent_from_near_names_and_content():
    from tools.benchmark.retrieval import source_oracle as so

    files = {
        "original.go": b"package p\nfunc render() {}\n",
        "neighbor.go": b"package p\nfunc rendor() {}\n",
        "mention.go": b"package p\n// render, rendor and rendar are mentioned\n",
    }
    oracle = so.SourceOracleIndex(
        {path: (raw, ev.digest(raw)) for path, raw in files.items()}, {"render"}
    )
    partition = oracle.typo_gold_partition("go", "rendar", "render")
    assert partition["intended_base_files"] == ["original.go"]
    assert partition["near_declaration_names"] == ["render", "rendor"]
    assert partition["near_declaration_files"] == ["neighbor.go", "original.go"]
    assert partition["exact_content_collision_paths"] == ["mention.go"]
    assert partition["user_intent_state"] == "unjudged"
    exact_query = oracle.typo_gold_partition("go", "render", "rendor")
    assert exact_query["near_declaration_names"] == ["rendor"]
    assert exact_query["query_is_declaration_name"] is True
    with pytest.raises(so.SourceOracleError, match="not within one edit"):
        oracle.typo_gold_partition("go", "rendar", "unrelated")


def test_typo_partition_accepts_query_proven_parser_refusal():
    from tools.benchmark.retrieval import source_oracle as so

    files = {
        "main.go": b"package p\nfunc Param() {}\n",
        "broken.go": b"package p\nfunc Broken(\n// xParanY is a longer token\n",
    }
    oracle = so.SourceOracleIndex(
        {path: (raw, ev.digest(raw)) for path, raw in files.items()},
        {"Param", "Paran"},
        declaration_exclusions={
            (so.GO_EXACT_LOCAL_NAME, "Param"): {"broken.go"},
            (so.GO_NAME_OSA1_CASEFOLD, "Paran"): {"broken.go"},
        },
    )
    partition = oracle.typo_gold_partition("go", "Paran", "Param")
    assert partition["intended_base_files"] == ["main.go"]
    assert partition["near_declaration_names"] == ["Param"]
    assert partition["query_is_declaration_name"] is False
    files["broken.go"] = b"package p\nfunc Broken(\n// Paran is a full token\n"
    oracle = so.SourceOracleIndex(
        {path: (raw, ev.digest(raw)) for path, raw in files.items()},
        {"Param", "Paran"},
        declaration_exclusions={
            (so.GO_EXACT_LOCAL_NAME, "Param"): {"broken.go"},
            (so.GO_NAME_OSA1_CASEFOLD, "Paran"): {"broken.go"},
        },
    )
    with pytest.raises(so.SourceOracleError, match="may contain a query match"):
        oracle.typo_gold_partition("go", "Paran", "Param")


def test_osa1_text_exclusion_trigram_filter_preserves_regex_witnesses(monkeypatch):
    from tools.benchmark.retrieval import source_oracle as so

    query = "abcdefghijkl"
    one_edits = {query}
    for index in range(len(query)):
        one_edits.add(query[:index] + query[index + 1 :])
        one_edits.add(query[:index] + "x" + query[index + 1 :])
        if index + 1 < len(query):
            one_edits.add(query[:index] + query[index + 1] + query[index] + query[index + 2 :])
    for index in range(len(query) + 1):
        one_edits.add(query[:index] + "x" + query[index:])
    for name in sorted(one_edits):
        raw = f"prefix {name.upper()} suffix".encode()
        assert not so.declaration_query_textually_excluded(raw, query, "osa1_casefold")
    # The casefolded Unicode character is not an ASCII identifier name, so
    # this text cannot witness the folded typo declaration contract.
    assert so.declaration_query_textually_excluded("abcdefghijKl".encode(), query, "osa1_casefold")

    for raw in (b"abc" + b"x" * 20_000 + b"jkl", b"abxxefghijkl", b"nothing here"):
        regex_absent = so._osa1_text_pattern(query).search(raw.decode()) is None
        assert so.declaration_query_textually_excluded(raw, query, "osa1") == regex_absent
        assert so.declaration_query_textually_excluded(raw, query, "osa1_casefold")

    assert so.declaration_query_textually_excluded(
        b"prefix_abcxefghijkl_suffix", query, "osa1_casefold"
    )
    assert not so.declaration_query_textually_excluded(
        b"prefix abcxefghijkl suffix", query, "osa1_casefold"
    )
    assert not so.declaration_query_textually_excluded(
        b"prefix ABCDEFGHIJKL suffix", query, "osa1_casefold"
    )

    def unexpected_regex(_query):
        pytest.fail("the sparse source should be rejected before regex search")

    monkeypatch.setattr(so, "_osa1_text_pattern", unexpected_regex)
    assert so.declaration_query_textually_excluded(b"abc" + b"x" * 20_000 + b"jkl", query, "osa1")


def test_osa1_text_anchor_scan_matches_unanchored_reference():
    import random

    from tools.benchmark.retrieval import source_oracle as so

    generator = random.Random(20261003)
    for size in range(3, 13):
        for _ in range(200):
            query = "".join(generator.choices("abc", k=size))
            text = "".join(generator.choices("abcx\n", k=generator.randrange(48)))
            expected = so._osa1_text_pattern(query).search(text) is not None
            assert so._osa1_text_witness(text, query) == expected, (query, text)


def test_osa1_text_anchor_scan_does_not_search_entire_sparse_text(monkeypatch):
    from tools.benchmark.retrieval import source_oracle as so

    original = so._osa1_text_pattern("abcdefgh")
    starts = []

    class AnchoredPattern:
        def search(self, _text):
            pytest.fail("long-name witness search must use anchored candidates")

        def match(self, text, start):
            starts.append(start)
            return original.match(text, start)

    monkeypatch.setattr(so, "_osa1_text_pattern", lambda _query: AnchoredPattern())
    assert not so._osa1_text_witness("abc" + "x" * 20_000 + "fgh", "abcdefgh")
    assert len(starts) <= 5


def test_paired_typo_builder_emits_operation_and_stress_suites(tmp_path):
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    repo, baseline = _robustness_baseline(tmp_path)
    first, census = irs.derive_paired_full(repo, baseline, seed=7)
    again, other = irs.derive_paired_full(repo, baseline, seed=7)
    assert ev.canonical(census) == ev.canonical(other)
    assert {lane: ev.canonical(pair) for lane, pair in first.items()} == {
        lane: ev.canonical(pair) for lane, pair in again.items()
    }
    assert set(first) == {
        "clean",
        *(f"typo-{op}" for op in (*irs.TYPO_OPERATIONS, *irs.STRESS_TYPO_LANES)),
        *(f"default-typo-{op}" for op in irs.TYPO_OPERATIONS),
    }
    assert len(first["clean"][0]["tasks"]) == 6
    assert first["clean"][0]["routes"] == ["lexical", "semble-lexical-file"]
    assert len(census["lanes"]["clean"]["records"]) == 6
    two_edit = census["two_substitution_stress"]
    assert two_edit["product_request_mode"] == "unsupported"
    assert two_edit["admitted_candidates"] > 0
    assert len(two_edit["records"]) == 6
    assert all(row["user_intent_state"] == "unjudged" for row in two_edit["records"])
    assert all(
        sum(a != b for a, b in zip(row["base_query"].casefold(), row["query"].casefold())) == 2
        for row in two_edit["records"]
        if row["query"] is not None
    )
    assert all(
        not ev.source_oracle.osa_distance_at_most_one(
            row["base_query"].casefold(), row["query"].casefold()
        )
        for row in two_edit["records"]
        if row["query"] is not None
    )
    assert all(
        "near_name_collision" not in row["strata"] for row in census["lanes"]["clean"]["records"]
    )
    assert all(
        row["strata"]["length"] in {"short_1_6", "medium_7_16", "long_17_plus"}
        for row in census["lanes"]["clean"]["records"]
    )
    clean_families = {row["query_family_id"] for row in first["clean"][0]["tasks"]}
    for operation in (*irs.TYPO_OPERATIONS, *irs.STRESS_TYPO_LANES):
        lane = f"typo-{operation}"
        suite = first[lane][0]
        assert suite["routes"] == ["lexical"]
        assert suite["suite_id"].endswith(f"{lane}-casefold-v2-seed7")
        assert {row["query_family_id"] for row in suite["tasks"]} <= clean_families
        assert all(
            row["evaluation_contract"]["request_mode"] == "explicit_osa1_typo"
            for row in suite["tasks"]
        )
        assert all(
            row["source_partition"]["user_intent_state"] == "unjudged"
            for row in census["lanes"][lane]["records"]
            if row["status"] == "admitted"
        )
        if operation in irs.TYPO_OPERATIONS:
            default_lane = "default-" + lane
            default_suite = first[default_lane][0]
            assert default_suite["routes"] == ["lexical", "semble-lexical-file"]
            assert [row["query"] for row in default_suite["tasks"]] == [
                row["query"] for row in suite["tasks"]
            ]
            assert [row["file_judgments"] for row in default_suite["tasks"]] == [
                row["file_judgments"] for row in suite["tasks"]
            ]
            assert all(
                row["evaluation_contract"]["request_mode"] == "default_file_search"
                for row in default_suite["tasks"]
            )
            assert census["lanes"][default_lane] == {
                "derived_from": lane,
                "product_request_mode": "default_file_search",
                "admitted": len(default_suite["tasks"]),
            }


def test_paired_typo_oracle_limit_counts_intended_names_once(tmp_path, monkeypatch):
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    repo, baseline = _robustness_baseline(tmp_path)
    monkeypatch.setattr(ev.source_oracle, "MAX_QUERIES", len(baseline["tasks"]))
    suites, _census = irs.derive_paired_full(repo, baseline, seed=7)
    assert len(suites["clean"][0]["tasks"]) == len(baseline["tasks"])
    assert suites["typo-insertion"][0]["tasks"]


def test_stress_typo_generators_are_deterministic_one_edit_and_boundary_scoped():
    from tools.benchmark.retrieval import identifier_robustness_suite as irs
    from tools.benchmark.retrieval import source_oracle as so

    for lane, source in (
        ("keyboard", "readContext"),
        ("boundary", "readContext"),
        ("boundary", "read_context"),
    ):
        first = irs.propose_stress_typo(lane, source, 7, "family", 0)
        assert first == irs.propose_stress_typo(lane, source, 7, "family", 0)
        candidate, metadata = first
        assert candidate != source
        assert so.IDENTIFIER.fullmatch(candidate)
        assert so.osa_distance_at_most_one(source.casefold(), candidate.casefold())
        assert metadata["operation"] in {
            "keyboard_substitution",
            "camel_boundary_transposition",
            "snake_boundary_deletion",
        }
    assert irs.propose_stress_typo("boundary", "plain", 7, "family", 0)[0] is None


def test_two_substitution_probe_is_two_edits_and_not_a_product_osa1_request():
    from tools.benchmark.retrieval import identifier_robustness_suite as irs
    from tools.benchmark.retrieval import source_oracle as so

    original = "readContext"
    query, metadata = irs.propose_two_substitutions(original, 7, "family", 0)
    assert (query, metadata) == irs.propose_two_substitutions(original, 7, "family", 0)
    assert so.IDENTIFIER.fullmatch(query)
    assert len(query) == len(original)
    assert sum(a != b for a, b in zip(original.casefold(), query.casefold())) == 2
    assert not so.osa_distance_at_most_one(original.casefold(), query.casefold())
    assert metadata["operation"] == "two_substitutions"
    assert irs.propose_two_substitutions("Go", 7, "family", 0)[0] is None


def test_identifier_robustness_builder_is_deterministic_and_evaluator_bound(tmp_path):
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    repo, baseline = _robustness_baseline(tmp_path)
    first, census = irs.derive(repo, copy.deepcopy(baseline), seed=7, sample_size=3, no_answer=2)
    second, again = irs.derive(repo, copy.deepcopy(baseline), seed=7, sample_size=3, no_answer=2)
    assert ev.canonical(census) == ev.canonical(again)
    assert {lane: ev.canonical(pair) for lane, pair in first.items()} == {
        lane: ev.canonical(pair) for lane, pair in second.items()
    }
    assert list(first) == [
        "prefix",
        "infix",
        "components",
        "typo",
        "no-answer",
        "no-answer-content",
        "typo-content-absence",
    ]
    no_answer_lanes = {"no-answer", "no-answer-content", "typo-content-absence"}
    for lane, (suite, pack) in first.items():
        assert suite["routes"] == ["lexical"]
        assert all(set(task) == {"task_id", "query", "query_sha256"} for task in pack["tasks"])
        for task in suite["tasks"]:
            assert task["answerable"] is (lane not in no_answer_lanes)
        for record in census["lanes"][lane]["records"]:
            if lane not in no_answer_lanes and record["status"] == "admitted":
                assert record["base_name_in_gold"] is True
    assert set(census["lanes"]["components"]["ineligible"]) <= {"single_component"}
    near_miss = census["lanes"]["typo-content-absence"]
    assert near_miss["derived_from"] == "typo"
    assert near_miss["source_admitted"] == census["lanes"]["typo"]["admitted"]
    assert near_miss["admitted"] + near_miss["excluded"] == near_miss["source_admitted"]
    assert {row["source_task_id"] for row in near_miss["records"]}.isdisjoint(
        {row["source_task_id"] for row in near_miss["excluded_probes"]}
    )
    from tools.benchmark.retrieval.identifier_robustness_report import (
        verify_census_against_source,
    )

    verify_census_against_source(
        repo, first["typo-content-absence"][0], census, "typo-content-absence"
    )
    altered_census = copy.deepcopy(census)
    altered_census["lanes"]["typo-content-absence"]["records"][0]["source_task_id"] = "TYP-unknown"
    with pytest.raises(ValueError, match="typo absence admitted source mismatch"):
        verify_census_against_source(
            repo, first["typo-content-absence"][0], altered_census, "typo-content-absence"
        )
    suite, _pack = first["prefix"]
    tampered = copy.deepcopy(suite)
    tampered["tasks"][0]["file_judgments"] = []
    tampered["tasks"][0]["answerable"] = False
    tampered["tasks"][0]["gold"] = []
    with pytest.raises(ev.EvidenceError, match="source oracle judgments differ"):
        ev.validate_suite(repo, tampered)
    renamed = copy.deepcopy(suite)
    renamed["tasks"][0]["source_oracle"]["contract"] = ev.source_oracle.GO_EXACT_LOCAL_NAME
    with pytest.raises(ev.EvidenceError):
        ev.validate_suite(repo, renamed)


def test_go_name_variants_distinguish_prefix_infix_and_component_position(tmp_path):
    so = ev.source_oracle
    oracle = _variant_oracle(tmp_path)
    # A fragment inside names is an infix but never a prefix.
    assert oracle.matched_names(so.GO_NAME_PREFIX, "ind") == []
    assert "BindJSON" in oracle.matched_names(so.GO_NAME_INFIX, "ind")
    # Infix also admits the fragment at position zero.
    assert "BindJSON" in oracle.matched_names(so.GO_NAME_INFIX, "Bin")
    # Components match a contiguous run at any position, not only the head.
    assert oracle.matched_names(so.GO_NAME_COMPONENTS, "html page") == ["render_html_page"]
    assert so.has_inferred_acronym_boundary("prepareTrustedCIDRs")
    assert so.has_inferred_acronym_boundary("TestLoadHTMLFSTestMode")
    assert not so.has_inferred_acronym_boundary("render_html_page")
    assert not so.has_inferred_acronym_boundary("ShouldBind")


def test_go_name_variants_decode_non_ascii_declarations_without_crashing(tmp_path):
    so = ev.source_oracle
    oracle = _variant_oracle(tmp_path, {"cafe.go": "package sample\nfunc Café() {}\n".encode()})
    assert oracle.matched_names(so.GO_NAME_PREFIX, "Caf") == ["Café"]
    assert "Café" in oracle.matched_names(so.GO_NAME_OSA1, "Cafe")
    assert so.name_components("Café") == ()
    assert oracle.matched_names(so.GO_NAME_COMPONENTS, "bind json") == ["BindJSON", "BindJSONTest"]


def test_identifier_robustness_builder_guards_typo_infix_and_no_answer(tmp_path, monkeypatch):
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    for attempt in range(64):
        query, meta = irs.propose("infix", "StatusStatus", 3, "fam", attempt)
        assert query is None or not "StatusStatus".startswith(query)
        assert query is not None or meta.get("retry") == "infix_equals_prefix"
    repo, baseline = _robustness_baseline(tmp_path)
    original = irs.propose

    def forced(lane, name, seed, family, attempt):
        if lane == "typo" and name == "BindJSON":
            return "ReadJSON", {"operation": "forced"}  # another declaration: collision
        if lane == "typo" and name == "BindXML":
            return "2indXML", {"operation": "forced"}  # product request rejects digit-leading
        return original(lane, name, seed, family, attempt)

    monkeypatch.setattr(irs, "propose", forced)
    monkeypatch.setattr(
        irs, "_no_answer_probes", lambda *_args: [("BindYAML", {}), ("QuuxZorp", {})]
    )
    _suites, census = irs.derive(repo, copy.deepcopy(baseline), seed=7, sample_size=6, no_answer=2)
    typo = {r["base_query"]: r for r in census["lanes"]["typo"]["records"]}
    assert typo["BindJSON"]["status"] == "excluded_exact_name_collision"
    assert typo["BindXML"]["status"] == "ineligible"
    assert typo["BindXML"]["generation"]["ineligible"] == "outside_folded_typo_request"
    probe = census["lanes"]["no-answer"]["records"][0]
    # "BindYAML" appears only in a comment: no declaration, one content file.
    assert probe["answer_class"] == "no_answer"
    assert probe["content_word_files"] == 1
    assert probe["content_substring_files"] == 1
    components = census["lanes"]["components"]["records"]
    assert {r["base_query"] for r in components if r["status"] == "admitted"} == {
        "BindJSON",
        "BindXML",
        "ReadJSON",
        "bindQuery",
    }
    assert {
        r["base_query"]: r["generation"]["ineligible"]
        for r in components
        if r["status"] == "ineligible"
    } == {"HTMLRender": "ambiguous_acronym_boundary", "Render": "single_component"}


def test_identifier_robustness_sample_depends_on_seed_only():
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    tasks = [{"query_family_id": f"fam-{index}"} for index in range(200)]
    first = irs.sample_families(tasks, 1, 20)
    assert first == irs.sample_families(list(reversed(tasks)), 1, 20)
    assert first != irs.sample_families(tasks, 2, 20)


def test_identifier_robustness_content_no_answer_lane_excludes_present_bytes(tmp_path, monkeypatch):
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    repo, baseline = _robustness_baseline(tmp_path)
    probes = [("BindYAML", {}), ("QuuxZorp", {}), ("BindJ", {}), ("WidgetPlume", {})]
    monkeypatch.setattr(irs, "_no_answer_probes", lambda *_args: probes)
    suites, census = irs.derive(repo, copy.deepcopy(baseline), seed=7, sample_size=6, no_answer=4)
    again_suites, again = irs.derive(
        repo, copy.deepcopy(baseline), seed=7, sample_size=6, no_answer=4
    )
    assert ev.canonical(census) == ev.canonical(again)
    assert ev.canonical(suites) == ev.canonical(again_suites)
    assert list(suites)[-3:] == ["no-answer", "no-answer-content", "typo-content-absence"]
    declaration, _pack = suites["no-answer"]
    # The declaration-intent lane keeps every probe, including those present as content.
    assert [t["task_id"] for t in declaration["tasks"]] == [
        "NOA-001",
        "NOA-002",
        "NOA-003",
        "NOA-004",
    ]
    content, pack = suites["no-answer-content"]
    assert content["suite_id"].endswith("-robustness-no-answer-content-v2-seed7")
    assert [(t["task_id"], t["query"]) for t in content["tasks"]] == [
        ("NOC-002", "QuuxZorp"),
        ("NOC-004", "WidgetPlume"),
    ]
    assert [t["task_id"] for t in pack["tasks"]] == ["NOC-002", "NOC-004"]
    by_query = {t["query"]: t for t in declaration["tasks"]}
    for task in content["tasks"]:
        source = by_query[task["query"]]
        assert {
            **task,
            "task_id": source["task_id"],
            "source_oracle": source["source_oracle"],
        } == source
        assert task["answerable"] is False and task["file_judgments"] == []
        assert task["source_oracle"]["contract"] == ev.source_oracle.ASCII_CONTENT_ABSENT_CASEFOLD
        assert source["source_oracle"]["contract"] == ev.source_oracle.GO_EXACT_LOCAL_NAME
    lane = census["lanes"]["no-answer-content"]
    assert lane["admitted"] == 2 and lane["source_admitted"] == 4 and lane["excluded"] == 2
    # "BindYAML" occurs only in a comment; "BindJ" is also a declaration infix.
    assert lane["excluded_probes"] == [
        {"source_task_id": "NOA-001", "query": "BindYAML", "reasons": ["content_bytes_present"]},
        {
            "source_task_id": "NOA-003",
            "query": "BindJ",
            "reasons": ["content_bytes_present", "declaration_infix_present"],
        },
    ]
    assert lane["excluded_reasons"] == {"content_bytes_present": 2, "declaration_infix_present": 1}
    assert lane["records"] == [
        {"task_id": "NOC-002", "source_task_id": "NOA-002"},
        {"task_id": "NOC-004", "source_task_id": "NOA-004"},
    ]
    assert set(census["lanes"]["no-answer"]["records"][0]) >= {"content_substring_files"}
    assert "derived_from" not in census["lanes"]["no-answer"]
    monkeypatch.setattr(irs, "_no_answer_probes", lambda *_args: probes[:1])
    with pytest.raises(ev.EvidenceError, match="no content-absent probes were admitted"):
        irs.derive(repo, copy.deepcopy(baseline), seed=7, sample_size=6, no_answer=1)


def test_identifier_robustness_content_no_answer_lane_excludes_case_variants(tmp_path, monkeypatch):
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    repo, baseline = _robustness_baseline(tmp_path)
    # "Readjson" is not a declaration byte-for-byte, but case-folds to ReadJSON.
    probes = [("Readjson", {}), ("QuuxZorp", {})]
    monkeypatch.setattr(irs, "_no_answer_probes", lambda *_args: probes)
    suites, census = irs.derive(repo, copy.deepcopy(baseline), seed=7, sample_size=6, no_answer=2)
    assert [t["query"] for t in suites["no-answer"][0]["tasks"]] == ["Readjson", "QuuxZorp"]
    assert [t["query"] for t in suites["no-answer-content"][0]["tasks"]] == ["QuuxZorp"]
    lane = census["lanes"]["no-answer-content"]
    assert lane["excluded_probes"] == [
        {
            "source_task_id": "NOA-001",
            "query": "Readjson",
            "reasons": ["content_bytes_present_casefold", "declaration_infix_present_casefold"],
        }
    ]


def test_content_no_answer_replay_refuses_content_positive_query(tmp_path, monkeypatch):
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    repo, baseline = _robustness_baseline(tmp_path)
    probes = [("BindYAML", {}), ("Readjson", {}), ("QuuxZorp", {})]
    monkeypatch.setattr(irs, "_no_answer_probes", lambda *_args: probes)
    suites, _census = irs.derive(repo, copy.deepcopy(baseline), seed=7, sample_size=6, no_answer=3)
    declaration = suites["no-answer"][0]
    content = suites["no-answer-content"][0]
    assert [task["query"] for task in content["tasks"]] == ["QuuxZorp"]
    ev.validate_suite(repo, declaration)
    ev.validate_suite(repo, content)
    schema_path = Path(__file__).parents[2] / "benchmark/retrieval/suite.schema.json"
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    jsonschema.validate(content, schema)
    assert all(
        task["source_oracle"]["contract"] == ev.source_oracle.GO_EXACT_LOCAL_NAME
        for task in declaration["tasks"]
    )

    legacy_content = copy.deepcopy(content)
    legacy_content["suite_id"] = content["suite_id"].replace(
        "-no-answer-content-v2-", "-no-answer-content-"
    )
    legacy_content["tasks"][0]["source_oracle"]["contract"] = ev.source_oracle.GO_EXACT_LOCAL_NAME
    assert legacy_content["suite_id"] != content["suite_id"]
    ev.validate_suite(repo, legacy_content)

    wrong_unit = copy.deepcopy(content)
    wrong_unit["tasks"][0]["source_oracle"]["unit"] = "symbol"
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(wrong_unit, schema)
    with pytest.raises(ev.EvidenceError, match="judgment unit mismatch"):
        ev.validate_suite(repo, wrong_unit)

    for query in ("BindYAML", "Readjson"):
        altered = copy.deepcopy(content)
        altered["tasks"][0]["query"] = query
        altered["tasks"][0]["query_sha256"] = ev.digest(query.encode())
        with pytest.raises(ev.EvidenceError, match="content absent"):
            ev.validate_suite(repo, altered)
