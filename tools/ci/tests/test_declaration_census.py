"""Hand-written declaration expectations bind both censuses and the name contracts.

`fixtures/declaration_census/expected.json` lists (line, declared name) pairs
written from the language rules, not from either parser's output. Comments,
docstrings, strings, macro bodies, variables, fields, namespaces and anonymous
or expression forms are deliberate decoys.
"""

from __future__ import annotations

import json
import re
import shutil
from pathlib import Path

import pytest

from tools.benchmark.retrieval import declaration_census_audit as audit
from tools.benchmark.retrieval import source_oracle as so

FIXTURES = Path(__file__).parent / "fixtures" / "declaration_census"
EXPECTED = json.loads((FIXTURES / "expected.json").read_text())
LANGUAGES = {
    "rust.rs": "rust",
    "python.py": "python",
    "typescript.ts": "typescript",
    "component.tsx": "typescript",
    "javascript.js": "javascript",
    "app.jsx": "javascript",
    "go.go": "go",
}
TOOLS = {"rust": "cargo", "go": "go", "typescript": "npm", "javascript": "npm"}


def expected_spans(name: str) -> set[tuple[int, bytes]]:
    raw = (FIXTURES / name).read_bytes()
    lines = raw.split(b"\n")
    spans = set()
    for line_number, token in EXPECTED[name]:
        text = lines[line_number - 1].decode("utf-8")
        matches = list(re.finditer(rf"(?<![\w#$]){re.escape(token)}(?![\w$])", text))
        assert len(matches) == 1, (name, line_number, token)
        start = sum(len(line) + 1 for line in lines[: line_number - 1])
        start += len(text[: matches[0].start()].encode("utf-8"))
        spans.add((start, token.encode("utf-8")))
    assert len(spans) == len(EXPECTED[name])
    return spans


@pytest.mark.parametrize("name", sorted(LANGUAGES))
def test_tree_sitter_census_matches_hand_written_declarations(name):
    raw = (FIXTURES / name).read_bytes()
    rows = so.declaration_census(LANGUAGES[name], name, raw)
    assert {(start, raw[start:end]) for start, end, *_definition in rows} == expected_spans(name)
    assert all(
        definition_start <= start < end <= definition_end
        for start, end, definition_start, definition_end, _kind in rows
    )


def require_tool(language: str) -> None:
    if language != "python" and shutil.which(TOOLS[language]) is None:
        pytest.skip(f"independent {language} checker toolchain is not installed")


@pytest.mark.parametrize("name", sorted(LANGUAGES))
def test_independent_parser_matches_hand_written_declarations(name, tmp_path, monkeypatch):
    language = LANGUAGES[name]
    require_tool(language)
    view = tmp_path / "view"
    view.mkdir()
    shutil.copyfile(FIXTURES / name, view / name)
    result = audit.audit_files(language, view, [name])
    assert result["status"] == "admitted", result
    assert result["agreeing_declarations"] == len(EXPECTED[name])
    assert result["checker"]["id"] == audit.CHECKER_IDS[language]
    if language == "python":
        assert audit._python_ast((FIXTURES / name).read_bytes()) == expected_spans(name)


MALFORMED = {
    "rust": ("broken.rs", b"fn broken( {\n"),
    "python": ("broken.py", b"def broken(:\n    pass\n"),
    "typescript": ("broken.ts", b"function broken( {\n"),
    "javascript": ("broken.js", b"class { broken\n"),
    "go": ("broken.go", b"package p\nfunc broken( {\n"),
}


@pytest.mark.parametrize("language", sorted(MALFORMED))
def test_malformed_source_refuses_census_instead_of_empty_gold(language, tmp_path):
    name, raw = MALFORMED[language]
    with pytest.raises(so.SourceOracleError, match="parse error"):
        so.declaration_census(language, name, raw)
    oracle = so.SourceOracleIndex({name: (raw, "0" * 64)}, {"broken"})
    contract = so.GO_EXACT_LOCAL_NAME if language == "go" else f"{language}_exact_local_name_v1"
    with pytest.raises(so.SourceOracleError, match="parse error"):
        oracle.expected_rows(contract, "broken", "distinct_file")
    assert oracle.census_failures(language)[0]["path"] == name
    require_tool(language)
    view = tmp_path / "view"
    view.mkdir()
    (view / name).write_bytes(raw)
    result = audit.audit_files(language, view, [name])
    assert result["status"] == "unsupported"
    assert [side for side, _reason in result["refused"][0]["refusals"]] == [
        "tree_sitter",
        "independent",
    ]


def test_audit_reports_a_single_parser_disagreement(tmp_path, monkeypatch):
    view = tmp_path / "view"
    view.mkdir()
    shutil.copyfile(FIXTURES / "python.py", view / "python.py")
    original = so.declaration_census

    def drop_one(language, path, raw):
        return original(language, path, raw)[1:]

    monkeypatch.setattr(so, "declaration_census", drop_one)
    result = audit.audit_files("python", view, ["python.py"])
    assert result["status"] == "unsupported"
    assert result["disagreements"][0]["only_tree_sitter"] == []
    assert len(result["disagreements"][0]["only_independent"]) == 1


def test_audit_without_files_of_the_language_is_not_admitted(tmp_path):
    view = tmp_path / "view"
    view.mkdir()
    shutil.copyfile(FIXTURES / "python.py", view / "python.py")
    result = audit.audit_files("python", view, ["python.py", "notes.md"])
    assert result["files"] == 1
    empty = audit.audit_files("python", view, [])
    assert empty["status"] == "unsupported" and empty["files"] == 0


def test_partial_census_keeps_file_inventory_and_query_eligibility_explicit(tmp_path):
    good = b"def target():\n    pass\n"
    broken = b"def broken(:\n    pass\n"
    view = tmp_path / "view"
    view.mkdir()
    (view / "good.py").write_bytes(good)
    (view / "broken.py").write_bytes(broken)
    audit_result = audit.audit_files("python", view, ["good.py", "broken.py"])
    assert audit_result["status"] == "unsupported"
    assert audit_result["files"] == 2
    assert audit_result["admitted_paths"] == ["good.py"]
    assert audit_result["excluded_paths"] == ["broken.py"]
    assert audit_result["file_set_sha256"] == audit.file_set_sha256(
        "python", {"good.py": good, "broken.py": broken}
    )

    exact = "python_exact_local_name_v1"
    prefix = "python_declaration_name_prefix_v1"
    files = {"good.py": (good, "good-digest"), "broken.py": (broken, "broken-digest")}
    oracle = so.SourceOracleIndex(
        files,
        {"target", "missing", "tar", "broken"},
        {
            (exact, "target"): {"broken.py"},
            (exact, "missing"): {"broken.py"},
            (prefix, "tar"): {"broken.py"},
        },
    )
    assert set(oracle.files) == {"good.py", "broken.py"}
    assert oracle.expected_rows(exact, "target", "distinct_file") == [
        {"path": "good.py", "file_sha256": "good-digest", "grade": 3}
    ]
    assert oracle.expected_rows(exact, "missing", "distinct_file") == []
    assert oracle.expected_rows(prefix, "tar", "distinct_file") == [
        {"path": "good.py", "file_sha256": "good-digest", "grade": 3}
    ]
    assert oracle.expected_rows(so.ASCII_IDENTIFIER_WORD, "broken", "distinct_file") == [
        {"path": "broken.py", "file_sha256": "broken-digest", "grade": 3}
    ]
    with pytest.raises(so.SourceOracleError, match="explicit query eligibility"):
        oracle.expected_rows(exact, "broken", "distinct_file")
    with pytest.raises(so.SourceOracleError, match="complete declaration census"):
        oracle.declared_names("python")


def test_partial_census_refuses_possible_match_and_parseable_file():
    exact = "python_exact_local_name_v1"
    broken = b"# target may be a declaration\ndef broken(:\n"
    oracle = so.SourceOracleIndex(
        {"broken.py": (broken, "broken-digest")},
        {"target"},
        {(exact, "target"): {"broken.py"}},
    )
    with pytest.raises(so.SourceOracleError, match="may contain a query match"):
        oracle.expected_rows(exact, "target", "distinct_file")
    with pytest.raises(so.SourceOracleError, match="outside its source language"):
        so.SourceOracleIndex(
            {"broken.py": (broken, "broken-digest")},
            {"target"},
            {(exact, "target"): {"other.py"}},
        )
    valid = b"def other():\n    pass\n"
    oracle = so.SourceOracleIndex(
        {"valid.py": (valid, "valid-digest")},
        {"target"},
        {(exact, "target"): {"valid.py"}},
    )
    with pytest.raises(so.SourceOracleError, match="complete census"):
        oracle.expected_rows(exact, "target", "distinct_file")


def test_partial_census_cache_is_keyed_by_each_query_exclusion_set():
    exact = "python_exact_local_name_v1"
    files = {
        "a.py": (b"def bad_a(:\n", "a-digest"),
        "b.py": (b"def bad_b(:\n", "b-digest"),
    }
    oracle = so.SourceOracleIndex(
        files,
        {"absent_a", "absent_b"},
        {
            (exact, "absent_a"): {"a.py", "b.py"},
            (exact, "absent_b"): {"a.py"},
        },
    )
    assert oracle.expected_rows(exact, "absent_a", "distinct_file") == []
    with pytest.raises(so.SourceOracleError, match="explicit query eligibility: b.py"):
        oracle.expected_rows(exact, "absent_b", "distinct_file")


def oracle_for(*names: str) -> so.SourceOracleIndex:
    return so.SourceOracleIndex(
        {name: ((FIXTURES / name).read_bytes(), name + "-digest") for name in names},
        {"placeholder"},
    )


def test_language_contracts_select_only_their_own_declarations():
    oracle = oracle_for("rust.rs", "python.py", "typescript.ts", "component.tsx", "javascript.js")
    # Exact names are per language: Rust and Python both declare `target`.
    assert oracle.matched_names("rust_exact_local_name_v1", "target") == ["target"]
    assert [
        row["path"]
        for row in oracle.expected_rows("python_exact_local_name_v1", "target", "distinct_file")
    ] == ["python.py"]
    assert len(oracle.declaration_name_spans("python_exact_local_name_v1", "target")) == 2
    # Rust symbol rows are definition spans containing the declared name bytes.
    rows = oracle.expected_rows("rust_exact_local_name_v1", "target", "symbol")
    raw = (FIXTURES / "rust.rs").read_bytes()
    assert [raw[row["start_byte"] : row["end_byte"]].split(b"(")[0] for row in rows] == [
        b"const fn target",
        b"pub fn target",
    ]
    # TypeScript covers .ts and .tsx; JavaScript does not see TypeScript files.
    assert oracle.matched_names("typescript_exact_local_name_v1", "render") == ["render"]
    assert oracle.matched_names("javascript_exact_local_name_v1", "render") == []
    assert oracle.matched_names("javascript_exact_local_name_v1", "target") == ["target"]
    # Decoys in comments, strings and macro bodies are never declarations.
    for contract in ("rust_exact_local_name_v1", "python_exact_local_name_v1"):
        for decoy in ("comment_decoy", "doc_decoy", "string_decoy", "hidden_in_macro", "HIDDEN"):
            assert oracle.matched_names(contract, decoy) == []
    assert oracle.matched_names("typescript_exact_local_name_v1", "NS") == []
    assert oracle.matched_names("typescript_exact_local_name_v1", "Named") == []


def test_language_name_variants_are_exhaustive_and_case_sensitive():
    oracle = oracle_for("rust.rs", "python.py", "typescript.ts")
    assert oracle.matched_names("rust_declaration_name_prefix_v1", "tar") == [
        "target",
        "target_async",
    ]
    assert oracle.matched_names("rust_declaration_name_prefix_v1", "Tar") == ["Target"]
    assert oracle.matched_names("rust_declaration_name_infix_v1", "atc") == ["r#match"]
    assert oracle.matched_names("python_declaration_name_infix_v1", "ecorate") == ["decorated"]
    assert oracle.matched_names("python_declaration_name_components_v1", "target async") == [
        "target_async"
    ]
    assert oracle.matched_names("python_declaration_name_components_v1", "async target") == []
    # A one-edit typo is entitled to every equally close declaration, never the exact name.
    assert oracle.matched_names("typescript_declaration_name_osa1_v1", "helpr") == ["helper"]
    assert oracle.matched_names("typescript_declaration_name_osa1_v1", "valeu") == ["value"]
    assert oracle.matched_names("rust_declaration_name_osa1_v1", "Targt") == ["Target"]
    assert oracle.matched_names("rust_declaration_name_osa1_v1", "target") == ["Target"]
    assert oracle.matched_names("typescript_declaration_name_infix_v1", "ecre") == ["#secret"]
    assert "café" in oracle.declared_names("python")


def test_every_language_has_complete_contract_inventory():
    for language in ("go", "rust", "python", "typescript", "javascript"):
        variants = {variant for lang, variant in so.NAME_CONTRACTS.values() if lang == language}
        assert variants == set(so.NAME_VARIANTS)
        assert language in so.DECLARATION_CENSUS and language in audit.CHECKER_IDS


VARIANT_PY = b"""class Binder:
    def bind_body(self):
        pass
def bind_json():
    pass
def bind_xml():
    pass
def bindQuery():
    pass
class HTMLRender:
    def render(self):
        pass
def render_html_page():
    pass
def read_json():
    pass
# bind_yaml is only mentioned in a comment.
"""


def python_robustness_baseline(tmp_path, extra: dict[str, bytes] | None = None):
    import subprocess

    from tools.benchmark.retrieval import evaluator as ev
    from tools.ci.tests.test_source_oracle_suite import _baseline

    files = {"variant.py": VARIANT_PY, **(extra or {})}
    repo = tmp_path / "robust-python"
    repo.mkdir()
    for name, raw in files.items():
        (repo / name).write_bytes(raw)
    git = ["git", "-C", str(repo)]
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run([*git, "add", *files], check=True)
    subprocess.run(
        [*git, "-c", "user.name=T", "-c", "user.email=t@e.invalid", "-c", "commit.gpgsign=false"]
        + ["commit", "-qm", "frozen"],
        check=True,
    )
    commit = subprocess.check_output([*git, "rev-parse", "HEAD"], text=True).strip()
    baseline = _baseline(commit, {"a.go": b"x\ny\n", "b.go": b"x\ny\nz\n"})
    universe = [
        {"path": path, "file_sha256": ev.digest(raw)} for path, raw in sorted(files.items())
    ]
    baseline.update(
        file_universe=universe, file_universe_digest=ev.universe_digest(universe), tasks=[]
    )
    lines = VARIANT_PY.splitlines(keepends=True)
    names = ["bind_json", "bind_xml", "bindQuery", "HTMLRender", "render", "read_json"]
    for index, name in enumerate(names, 1):
        line = next(
            i
            for i, text in enumerate(lines, 1)
            if re.search(rb"(?:def|class) " + name.encode() + rb"\b", text)
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
                        "path": "variant.py",
                        "start_byte": start,
                        "end_byte": start + len(lines[line - 1]),
                        "start_line": line,
                        "end_line": line,
                        "file_sha256": ev.digest(VARIANT_PY),
                        "block_sha256": ev.digest(lines[line - 1]),
                        "grade": 1,
                    }
                ],
            }
        )
    return repo, baseline


def test_python_robustness_suite_is_audited_and_evaluator_bound(tmp_path):
    import copy

    from tools.benchmark.retrieval import evaluator as ev
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    repo, baseline = python_robustness_baseline(tmp_path)
    first, census = irs.derive(repo, copy.deepcopy(baseline), 7, 3, 2, language="python")
    again, census_again = irs.derive(repo, copy.deepcopy(baseline), 7, 3, 2, language="python")
    assert ev.canonical(census) == ev.canonical(census_again)
    assert ev.canonical(first) == ev.canonical(again)
    assert census["language"] == "python"
    assert census["census_audit"]["status"] == "admitted"
    assert census["census_audit"]["checker"]["id"] == "cpython_ast"
    contracts = {lane: census["lanes"][lane]["contract"] for lane in irs.LANE_VARIANTS}
    assert contracts == {
        "prefix": "python_declaration_name_prefix_v1",
        "infix": "python_declaration_name_infix_v1",
        "components": "python_declaration_name_components_v1",
        "typo": "python_declaration_name_osa1_v1",
        "no-answer": "python_exact_local_name_v1",
    }
    for lane, (suite, _pack) in first.items():
        if lane in ("no-answer-content", "typo-content-absence"):
            for task in suite["tasks"]:
                assert task["source_oracle"]["contract"] == (
                    ev.source_oracle.ASCII_CONTENT_ABSENT_CASEFOLD
                    if lane == "no-answer-content"
                    else ev.source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD
                )
            continue
        for task in suite["tasks"]:
            assert task["source_oracle"]["contract"] == contracts[lane]
            assert task["query_family_id"].startswith(("fam-", "python-no-answer-"))
    components = {
        row["base_query"]: row["query"]
        for row in census["lanes"]["components"]["records"]
        if row["status"] == "admitted"
    }
    assert components.get("read_json", "read json") == "read json"
    suite, _pack = first["prefix"]
    tampered = copy.deepcopy(suite)
    tampered["tasks"][0]["file_judgments"] = []
    tampered["tasks"][0]["answerable"] = False
    tampered["tasks"][0]["gold"] = []
    with pytest.raises(ev.EvidenceError, match="source oracle judgments differ"):
        ev.validate_suite(repo, tampered)


def test_robustness_refuses_a_language_without_independent_census_agreement(tmp_path):
    from tools.benchmark.retrieval import evaluator as ev
    from tools.benchmark.retrieval import identifier_robustness_suite as irs

    # Python 2 syntax: CPython 3 `ast` refuses this file, so the language is unsupported.
    repo, baseline = python_robustness_baseline(
        tmp_path, {"legacy.py": b"def legacy():\n    print 'old'\n"}
    )
    with pytest.raises(ev.EvidenceError, match="not independently admitted"):
        irs.derive(repo, baseline, 7, 3, 2, language="python")
