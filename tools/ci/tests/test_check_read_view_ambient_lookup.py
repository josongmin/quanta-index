"""Focused P04 source scanner checks."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "lint/check-read-view-ambient-lookup.py"


def module():
    spec = importlib.util.spec_from_file_location("check_read_view_ambient_lookup", SCRIPT)
    assert spec and spec.loader
    loaded = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = loaded
    spec.loader.exec_module(loaded)
    return loaded


def fixture(root: Path, route: str, other: str = "pub fn healthy() {}\n") -> None:
    route_path = root / "crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs"
    route_path.parent.mkdir(parents=True)
    route_path.write_text(route, encoding="utf-8")
    (route_path.parent / "mod.rs").write_text("mod lexical;\n", encoding="utf-8")
    other_path = root / "crates/quanta-index-core/src/lib.rs"
    other_path.parent.mkdir(parents=True)
    other_path.write_text(other, encoding="utf-8")


@pytest.mark.parametrize(
    "route",
    [
        "fn query() { self .lex_opener.open(); }",
        "fn query() { self /* gap */ . lex_opener.open(); }",
        "fn query() { (self) . r#lex_opener.open(); }",
        "fn query() { self\n.\nlex_opener.open(); }",
        "fn query() { ((self)).lex_opener.open(); }",
        "fn query() { (&*self).lex_opener.open(); }",
        "fn query() { (&self).lex_opener.open(); }",
        "fn query() { (&mut self).lex_opener.open(); }",
    ],
)
def test_route_forbidden_field_spacing_and_comments_fail(tmp_path: Path, route: str):
    fixture(tmp_path, route)
    assert any("self.lex_opener" in issue for issue in module().scan(tmp_path))


def test_alias_to_dispatcher_cannot_hide_forbidden_route_field(tmp_path: Path):
    fixture(
        tmp_path,
        "fn query(&self) { let dispatcher = self; dispatcher.lex_opener.open(); }\n",
    )
    assert any("lex_opener" in issue for issue in module().scan(tmp_path))


def test_view_accessor_methods_are_not_ambient_fields(tmp_path: Path):
    fixture(
        tmp_path,
        "fn query(&self) { view.history_text()?; runtime_state.snapshots(); }\n",
    )
    assert module().scan(tmp_path) == []


def test_comments_and_literals_do_not_create_false_positives(tmp_path: Path):
    fixture(
        tmp_path,
        """// self .lex_opener and QueryReadViewV1
/* nested /* self.lex_opener */ and RepoMapQueryPort */
const STRING: &str = "self.lex_opener ReadIdentityV1";
const RAW: &str = r##"self . lex_opener read_query_snapshot"##;
const CHARACTER: char = 'R';
fn query() { let _ = "self.lex_opener"; }
""",
        '/// ReadIdentityV1\nconst OLD: &str = "RepoMapQueryPort";\n',
    )
    assert module().scan(tmp_path) == []


def test_retired_identifiers_in_live_code_fail(tmp_path: Path):
    fixture(tmp_path, "fn healthy() {}", "pub struct QueryReadViewV1;\n")
    assert any("QueryReadViewV1" in issue for issue in module().scan(tmp_path))


@pytest.mark.parametrize("route", ["", "/* unterminated"])
def test_empty_or_unscannable_source_fails(tmp_path: Path, route: str):
    fixture(tmp_path, route)
    assert module().scan(tmp_path)


def test_missing_route_tree_fails(tmp_path: Path):
    (tmp_path / "crates/quanta-index-core/src").mkdir(parents=True)
    assert module().scan(tmp_path)


def test_missing_route_inventory_fails(tmp_path: Path):
    fixture(tmp_path, "fn healthy() {}")
    route_dir = tmp_path / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    (route_dir / "mod.rs").unlink()
    assert any("mod.rs" in issue for issue in module().scan(tmp_path))


def test_missing_declared_route_module_fails(tmp_path: Path):
    fixture(tmp_path, "fn healthy() {}")
    route_dir = tmp_path / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    (route_dir / "mod.rs").write_text("mod lexical;\nmod semantic;\n", encoding="utf-8")
    assert any("semantic" in issue for issue in module().scan(tmp_path))


def test_route_inventory_follows_declared_modules(tmp_path: Path):
    fixture(tmp_path, "fn healthy() {}")
    route_dir = tmp_path / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    (route_dir / "lexical.rs").unlink()
    (route_dir / "mod.rs").write_text("mod semantic;\n", encoding="utf-8")
    (route_dir / "semantic.rs").write_text("fn healthy() {}\n", encoding="utf-8")
    assert module().scan(tmp_path) == []


def test_malformed_route_cannot_certify(tmp_path: Path):
    fixture(tmp_path, "fn query() {\n")
    assert any("cannot be parsed" in issue for issue in module().scan(tmp_path))


def test_unavailable_route_parser_cannot_certify(tmp_path: Path, monkeypatch):
    fixture(tmp_path, "fn healthy() {}\n")
    monkeypatch.setitem(sys.modules, "tree_sitter_language_pack", None)
    assert any("parser is unavailable" in issue for issue in module().scan(tmp_path))


def test_inline_module_resolves_nested_child(tmp_path: Path):
    fixture(tmp_path, "fn healthy() {}")
    route_dir = tmp_path / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    (route_dir / "mod.rs").write_text("mod nested { mod leaf; }\n", encoding="utf-8")
    nested = route_dir / "nested"
    nested.mkdir()
    (nested / "leaf.rs").write_text("fn healthy() {}\n", encoding="utf-8")
    assert module().scan(tmp_path) == []


def test_inline_module_missing_nested_child_fails(tmp_path: Path):
    fixture(tmp_path, "fn healthy() {}")
    route_dir = tmp_path / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    (route_dir / "mod.rs").write_text("mod nested { mod leaf; }\n", encoding="utf-8")
    assert any("leaf" in issue for issue in module().scan(tmp_path))


def test_file_stem_module_directory_resolves_child(tmp_path: Path):
    fixture(tmp_path, "mod child;\n")
    route_dir = tmp_path / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    nested = route_dir / "lexical"
    nested.mkdir()
    (nested / "child.rs").write_text("fn healthy() {}\n", encoding="utf-8")
    assert module().scan(tmp_path) == []


def test_literal_path_route_module_resolves_in_directory(tmp_path: Path):
    fixture(tmp_path, "fn healthy() {}")
    route_dir = tmp_path / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    (route_dir / "mod.rs").write_text('#[path = "actual.rs"] mod logical;\n', encoding="utf-8")
    (route_dir / "actual.rs").write_text("fn healthy() {}\n", encoding="utf-8")
    assert module().scan(tmp_path) == []


def test_path_from_a_route_file_uses_its_physical_parent(tmp_path: Path):
    fixture(tmp_path, '#[path = "sub/actual.rs"] mod logical;\n')
    route_dir = tmp_path / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    nested = route_dir / "sub"
    nested.mkdir()
    (nested / "actual.rs").write_text("mod leaf;\n", encoding="utf-8")
    (nested / "leaf.rs").write_text("fn healthy() {}\n", encoding="utf-8")
    assert module().scan(tmp_path) == []


def test_path_outside_route_directory_fails(tmp_path: Path):
    fixture(tmp_path, "fn healthy() {}")
    route_dir = tmp_path / "crates/quanta-index-search-plane/src/query_dispatcher/routes"
    (route_dir / "mod.rs").write_text('#[path = "../escape.rs"] mod logical;\n', encoding="utf-8")
    assert any("unsupported path" in issue for issue in module().scan(tmp_path))
