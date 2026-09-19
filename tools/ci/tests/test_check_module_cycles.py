"""Tests for tools/ci/lint/check-module-cycles.py."""

from __future__ import annotations

import importlib.util
import sys
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-module-cycles.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_module_cycles", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_module_cycles"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def crate(tmp_path: Path, files: dict[str, str], name: str = "demo") -> Path:
    root = tmp_path / name
    for rel, body in files.items():
        path = root / "src" / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(body), encoding="utf-8")
    return root


def keys(root: Path) -> list[str]:
    return [cycle.key for cycle in MODULE.find_cycles([root])]


def test_one_way_dependencies_have_no_cycle(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod a;\nmod b;\nmod c;\n",
            "a.rs": "use crate::b::B;\npub(crate) struct A(B);\n",
            "b.rs": "use crate::c::C;\npub(crate) struct B(C);\n",
            "c.rs": "pub(crate) struct C;\n",
        },
    )
    assert keys(root) == []


def test_two_modules_using_each_other_are_a_cycle(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod a;\nmod b;\n",
            "a.rs": "use crate::b::B;\npub(crate) struct A;\npub(crate) fn make() -> B { B }\n",
            "b.rs": "pub(crate) struct B;\npub(crate) fn wrap() -> crate::a::A { crate::a::A }\n",
        },
    )
    [cycle] = MODULE.find_cycles([root])
    assert cycle.key == "demo: a, b"
    assert cycle.links == ("a -> b", "b -> a")


def test_super_paths_are_edges(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod outer;\n",
            "outer/mod.rs": "mod left;\nmod right;\n",
            "outer/left.rs": "pub(crate) struct L;\npub(crate) fn r() -> super::right::R { super::right::R }\n",
            "outer/right.rs": "use super::left::L;\npub(crate) struct R;\npub(crate) fn l() -> L { L }\n",
        },
    )
    assert keys(root) == ["demo: outer::left, outer::right"]


def test_test_code_comments_and_strings_are_not_edges(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod a;\nmod b;\n#[cfg(test)]\nmod support;\n",
            "a.rs": """\
                use crate::b::B;
                pub(crate) struct A(B);
                """,
            "b.rs": """\
                //! See [`crate::a::A`] — an intra-doc link, not a dependency.
                pub(crate) struct B;
                // crate::a::A in a comment
                /* crate::a::A in a block /* nested */ comment */
                pub(crate) const NAME: &str = "crate::a::A";
                pub(crate) const RAW: &str = r#"crate::a::A "quoted""#;
                pub(crate) const CH: char = '{';

                #[cfg(test)]
                #[expect(clippy::unwrap_used, reason = "test")]
                mod tests {
                    use crate::a::A;
                }
                """,
            "support.rs": "use crate::a::A;\nuse crate::b::B;\n",
        },
    )
    assert keys(root) == []


def test_a_facade_re_export_resolves_to_the_defining_submodule(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod facade;\nmod user;\n",
            "facade/mod.rs": "mod inner;\npub(crate) use inner::Thing;\n",
            "facade/inner.rs": "pub(crate) struct Thing;\npub(crate) fn f() -> crate::user::U { crate::user::U }\n",
            "user.rs": "use crate::facade::Thing;\npub(crate) struct U;\npub(crate) fn t() -> Thing { Thing }\n",
        },
    )
    assert keys(root) == ["demo: facade::inner, user"]


def test_nested_use_trees_are_expanded(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod a;\nmod b;\nmod c;\n",
            "a.rs": "use crate::{b::{self, B}, c::C as Renamed};\npub(crate) struct A;\n",
            "b.rs": "pub(crate) struct B;\n",
            "c.rs": "pub(crate) struct C;\npub(crate) fn a() -> crate::a::A { crate::a::A }\n",
        },
    )
    assert keys(root) == ["demo: a, c"]


def test_a_module_and_its_submodules_are_one_owner(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod parent;\n",
            "parent.rs": "mod child;\npub(crate) struct P;\npub(crate) fn c() -> child::C { child::C }\n",
            "parent/child.rs": "use super::P;\npub(crate) struct C(P);\n",
        },
    )
    assert keys(root) == []


def test_the_baseline_tolerates_exactly_its_cycles(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod a;\nmod b;\nmod c;\n",
            "a.rs": "pub(crate) struct A;\npub(crate) fn b() -> crate::b::B { crate::b::B }\n",
            "b.rs": "pub(crate) struct B;\npub(crate) fn a() -> crate::a::A { crate::a::A }\n",
            "c.rs": "pub(crate) struct C;\n",
        },
    )
    baseline = tmp_path / "baseline.txt"
    baseline.write_text("# why\ndemo: a, b\n", encoding="utf-8")
    assert MODULE.check([root], baseline) == []

    # a cycle the baseline does not name fails
    baseline.write_text("# why\n", encoding="utf-8")
    [problem] = MODULE.check([root], baseline)
    assert problem.startswith("new cycle demo: a, b")

    # a baseline line whose cycle is gone fails, so the list only shrinks by an edit
    baseline.write_text("demo: a, b\ndemo: b, c\n", encoding="utf-8")
    [problem] = MODULE.check([root], baseline)
    assert problem.startswith("stale baseline line `demo: b, c`")


def test_a_grown_cycle_is_both_new_and_stale(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod a;\nmod b;\nmod c;\n",
            "a.rs": "pub(crate) struct A;\npub(crate) fn b() -> crate::b::B { crate::b::B }\n",
            "b.rs": "pub(crate) struct B;\npub(crate) fn c() -> crate::c::C { crate::c::C }\n",
            "c.rs": "pub(crate) struct C;\npub(crate) fn a() -> crate::a::A { crate::a::A }\n",
        },
    )
    baseline = tmp_path / "baseline.txt"
    baseline.write_text("demo: a, b\n", encoding="utf-8")
    problems = MODULE.check([root], baseline)
    assert any(p.startswith("new cycle demo: a, b, c") for p in problems)
    assert any(p.startswith("stale baseline line `demo: a, b`") for p in problems)


def test_update_baseline_keeps_the_header(tmp_path: Path):
    root = crate(
        tmp_path,
        {
            "lib.rs": "mod a;\nmod b;\n",
            "a.rs": "pub(crate) struct A;\npub(crate) fn b() -> crate::b::B { crate::b::B }\n",
            "b.rs": "pub(crate) struct B;\npub(crate) fn a() -> crate::a::A { crate::a::A }\n",
        },
    )
    baseline = tmp_path / "baseline.txt"
    baseline.write_text("# header line\n#\n# why a and b\nstale: x, y\n", encoding="utf-8")
    MODULE.update_baseline([root], baseline)
    assert baseline.read_text(encoding="utf-8") == "# header line\n#\n# why a and b\ndemo: a, b\n"
    assert MODULE.check([root], baseline) == []

