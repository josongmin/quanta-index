"""The parity report must read the live typed predicate registry, not old literals."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT = REPO_ROOT / "tools" / "benchmark" / "sourcegraph_parity.py"


def _module():
    spec = importlib.util.spec_from_file_location("sourcegraph_parity_inventory", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_filter_keywords_exclude_predicate_arguments_and_quoted_text():
    module = _module()
    query = 'repo:example (lang:rust OR case:yes) repo:has.file(path:src/lib.rs, "fork:no")'
    assert module.keywords_in(query) == {"repo", "lang", "case"}
    assert module.keywords_in('repo:has.file(path:src/lib.rs) "visibility:public"') == set()


def test_rust_query_witnesses_preserve_raw_text_and_ignore_comments():
    module = _module()
    source = r"""
        // client.sourcegraph("retired")
        /* client.sourcegraph("also retired") */
        client.sourcegraph(r#"file:has.content("quoted phrase") /\w+/"#);
        client.sourcegraph("type:commit \"fix bug\"");
        let error_label = r#"client.sourcegraph("diagnostic text")"#;
    """
    assert module.rust_string_literals(source, r"\.sourcegraph\(") == [
        r'file:has.content("quoted phrase") /\w+/',
        'type:commit "fix bug"',
    ]


def test_old_q_and_native_refusal_are_distinct_source_witnesses():
    module = _module()
    keywords, surfaces = {}, {}

    assert module.scan_old_q_queries(keywords, surfaces) > 0
    assert module.scan_native_repository_queries(keywords, surfaces) > 0
    assert surfaces["rev.at.time"].outcomes == {"old_q_assertion"}
    assert "native_assertion" in surfaces["repo.has.path"].outcomes
    assert "native_refusal" in keywords["visibility"].outcomes
    assert "native_assertion" not in keywords["repo"].outcomes


def test_check_keeps_the_generated_report_when_a_witness_source_disappears(
    monkeypatch,
    tmp_path,
    capsys,
):
    module = _module()
    report = tmp_path / "parity.md"
    report.write_text("previous inventory\n", encoding="utf-8")
    monkeypatch.setattr(module, "OUT_MD", report)
    monkeypatch.setattr(module, "RUNTIME_TESTS", tmp_path)
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--check", "--write"])

    assert module.main() == 1
    assert "FAIL: query witness inventory" in capsys.readouterr().err
    assert report.read_text(encoding="utf-8") == "previous inventory\n"


def test_typed_registry_inventory_is_complete():
    module = _module()
    canonical, aliases, route_owned = module.ours_predicates()

    assert set(canonical) == {
        "file.contains",
        "file.has.content",
        "file.has.contributor",
        "file.has.owner",
        "repo.has.commit.after",
        "repo.has.content",
        "repo.has.description",
        "repo.has.file",
        "repo.has.meta",
        "repo.has.topic",
    }
    assert set(aliases) == {
        "file.contains.content",
        "repo.contains.commit.after",
        "repo.contains.content",
        "repo.contains.file",
        "repo.contains.path",
        "repo.has.path",
    }
    assert route_owned == [
        "symbol.has.name",
        "symbol.local_name.exact",
        "symbol.qualified_name.exact",
    ]


def test_typed_registry_inventory_refuses_missing_rows_or_names():
    module = _module()
    registry = module.read(module.PREDICATE_REGISTRY_RS)
    core = module.read(module.CORE_PREDICATE_RS)

    missing_row = registry.replace(
        "predicate: LexicalPredicateV1::FileContains,",
        "// predicate: LexicalPredicateV1::FileContains,",
        1,
    )
    with pytest.raises(ValueError, match="rows are missing or ambiguous"):
        module._registry_variants(
            missing_row, "PREDICATE_REGISTRY", "PredicateSpec", "LexicalPredicateV1", "predicate"
        )

    missing_name = core.replace(
        'Self::RepoHasMeta => "repo.has.meta",',
        '// Self::RepoHasMeta => "repo.has.meta",',
        1,
    )
    with pytest.raises(ValueError, match="ALL/name tables are incomplete"):
        module._typed_names(missing_name, "LexicalPredicateV1")


@pytest.mark.parametrize("mutation", ["missing", "duplicate", "wrong_family"])
def test_predicate_inventory_rejects_family_drift(monkeypatch, mutation):
    module = _module()
    core = module.read(module.CORE_PREDICATE_RS)
    start = core.index("pub const fn family(self)")
    end = core.index("\n    }", start)
    family = core[start:end]
    if mutation == "missing":
        family = family.replace(" | Self::SymbolLocalNameExact", "", 1)
    elif mutation == "duplicate":
        family = family.replace("Self::SymbolLocalNameExact", "Self::SymbolHasName", 1)
    else:
        family = family.replace(
            "LexicalPredicateFamilyV1::Symbol", "LexicalPredicateFamilyV1::ContentOrRepo", 1
        )
    changed = core[:start] + family + core[end:]
    original_read = module.read
    monkeypatch.setattr(
        module,
        "read",
        lambda path: changed if path == module.CORE_PREDICATE_RS else original_read(path),
    )
    with pytest.raises(ValueError, match="family"):
        module.ours_predicates()
