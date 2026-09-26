"""Counterexamples for production Rust syntax and search-plane authority guards."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

from tree_sitter_language_pack import get_parser

SCRIPT = Path(__file__).resolve().parents[1] / "lint" / "check-rust-fallbacks.py"
SPEC = importlib.util.spec_from_file_location("check_rust_fallbacks", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
LINT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LINT)
PARSER = get_parser("rust")


def test_untracked_rust_source_is_scanned(tmp_path: Path, monkeypatch, capsys) -> None:
    source = tmp_path / "crates" / "quanta-index-search-plane" / "src" / "hidden.rs"
    source.parent.mkdir(parents=True)
    source.write_text("fn f() { let _ = Err::<(), ()>(()).or_else(|_| Ok(())); }\n")
    monkeypatch.setattr(LINT, "ROOT", tmp_path)
    assert LINT.main() == 1
    assert LINT.RULE_OR_ELSE in capsys.readouterr().out


def scan(
    source: str, *, include_debug: bool = False, include_search_plane: bool = False
) -> list[tuple[int, str]]:
    return LINT.scan_source(
        source.encode(),
        PARSER,
        include_debug=include_debug,
        include_search_plane=include_search_plane,
    )


class RustFallbackTest(unittest.TestCase):
    def test_three_replaced_rules_and_width_exceptions(self) -> None:
        source = """fn f(result: Result<(), ()>, value: u64) {
    if u8::try_from(value).is_ok() { narrow(); } else { wide(); }
    if u16::try_from(value).is_ok() { narrow(); } else { wide(); }
    if u32::try_from(value).is_ok() { narrow(); } else { wide(); }
    if result.is_ok() { serve(); } else { serve_default(); }
    if result.is_err() { serve_default(); } else { serve(); }
    if result.is_err() { return Err(()); }
    let _ = result.or_else(|_| Ok(()));
    let _ = result.or_else(|err| { log(err); Ok(()) });
    let _ = result.or_else(|_| { Ok(()) });
    let _ = result.or_else(|err| Err(err));
    let _ = "result.or_else(|_| Ok(()))";
    if value < 24 { tiny(); }
    else if u8::try_from(value).is_ok() { narrow(); }
    else if u16::try_from(value).is_ok() { medium(); }
    else if u32::try_from(value).is_ok() { large(); }
    else { wide(); }
    if (result.is_ok()) { serve(); } else { serve_default(); }
    if result.is_err() { serve_default(); } else if value > 0 { serve(); }
    if (u8::try_from(value).is_ok()) { narrow(); } else { wide(); }
    let _ = result.or_else(|_| { return Ok(()); });
    let _ = result.or_else(|_| {
        let nested = || -> Result<(), ()> { return Ok(()); };
        Err(())
    });
}
"""
        self.assertEqual(
            scan(source),
            [
                (5, LINT.RULE_IS_OK),
                (6, LINT.RULE_IS_ERR),
                (8, LINT.RULE_OR_ELSE),
                (9, LINT.RULE_OR_ELSE),
                (10, LINT.RULE_OR_ELSE),
                (18, LINT.RULE_IS_OK),
                (19, LINT.RULE_IS_ERR),
                (21, LINT.RULE_OR_ELSE),
            ],
        )

    def test_scan_scope(self) -> None:
        cases = {
            "crates/a/src/lib.rs": True,
            "crates/a/src/deep/lib.rs": True,
            "crates/a/fuzz/fuzz_targets/target.rs": True,
            "benchmarks/a/src/main.rs": True,
            "crates/a/tests/lib.rs": False,
            "crates/a/src/tests.rs": False,
            "crates/a/src/benches/lib.rs": False,
            "crates/a/examples/example.rs": False,
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertIs(LINT.in_scope(Path(path)), expected)

    def test_or_else_closure_variants(self) -> None:
        source = """fn f(result: Result<(), ()>) {
    let _ = result.or_else(move |_| Ok(()));
    let _ = result.or_else(|_| -> Result<(), ()> { Ok(()) });
    let _ = result.or_else(|error| Err(error));
    let _ = result.or_else(|_| (Ok(())));
    let _ = result.or_else((|_| Ok(())));
    let _ = result.or_else({ |_| Ok(()) });
    let _ = result.or_else::<_, ()>(|_| Ok(()));
}
"""
        self.assertEqual(
            scan(source),
            [(line, LINT.RULE_OR_ELSE) for line in (2, 3, 5, 6, 7, 8)],
        )

    def test_raw_identifiers_and_branch_tails_are_not_escape_hatches(self) -> None:
        source = """fn f(result: Result<(), ()>) {
    if result.r#is_ok() { serve(); } else { fallback(); }
    if result.r#is_err() { fallback(); } else { serve(); }
    let _ = result.r#or_else(|_| r#Ok(()));
    let _ = result.or_else(|_| if ready() { Ok(()) } else { Err(()) });
    let _ = result.or_else(|_| match ready() { true => Err(()), false => Ok(()) });
}
"""
        self.assertEqual(
            scan(source),
            [
                (2, LINT.RULE_IS_OK),
                (3, LINT.RULE_IS_ERR),
                (4, LINT.RULE_OR_ELSE),
                (5, LINT.RULE_OR_ELSE),
                (6, LINT.RULE_OR_ELSE),
            ],
        )

    def test_typed_and_qualified_result_ok(self) -> None:
        source = """fn f(result: Result<(), ()>) {
    let _ = result.or_else(|_| Ok::<(), ()>(()));
    let _ = result.or_else(|_| Result::Ok(()));
    let _ = result.or_else(|_| Result::<(), ()>::Ok(()));
    let _ = result.or_else(|_| std::result::Result::Ok(()));
    let _ = result.or_else(|_| core::result::Result::Ok(()));
    let _ = result.or_else(|_| Foo::Ok(()));
    let _ = result.or_else(|_| { return Result::Ok(()); });
}
"""
        self.assertEqual(
            scan(source),
            [(line, LINT.RULE_OR_ELSE) for line in (2, 3, 4, 5, 6, 8)],
        )

    def test_trailing_comments_do_not_hide_ok(self) -> None:
        source = """fn f(result: Result<(), ()>) {
    let _ = result.or_else(|_| { Ok(()) /* block comment */ });
    let _ = result.or_else(|_| { Ok(()) // line comment
    });
}
"""
        self.assertEqual(
            scan(source),
            [(2, LINT.RULE_OR_ELSE), (3, LINT.RULE_OR_ELSE)],
        )

    def test_comments_inside_predicates_and_arguments(self) -> None:
        source = """fn f(result: Result<(), ()>) {
    if (/* note */ result.is_ok()) { serve(); } else { fallback(); }
    if result.is_err(/* note */) { fallback(); } else { serve(); }
    if u8 /* note */ :: try_from(1_u64).is_ok() { narrow(); } else { wide(); }
    let _ = result.or_else(/* note */ |_| Ok(()));
    let _ = result.or_else(|_| { return /* note */ Ok(()); });
}
"""
        self.assertEqual(
            scan(source),
            [
                (2, LINT.RULE_IS_OK),
                (3, LINT.RULE_IS_ERR),
                (5, LINT.RULE_OR_ELSE),
                (6, LINT.RULE_OR_ELSE),
            ],
        )

    def test_parse_error_over_governed_syntax_fails_closed(self) -> None:
        with self.assertRaisesRegex(ValueError, "parse error"):
            scan("fn f() { let _ = bad.or_else(|_| Ok(()); }")

    def test_opaque_macro_fallback_is_not_silent_pass(self) -> None:
        for source in (
            "fn f(r: Result<(), ()>) { passthrough!{ r.or_else(|_| Ok(())) } }",
            "fn f(r: Result<(), ()>) { choice!{ if r.is_ok() { a(); } else { b(); } } }",
            "macro_rules! fallback { ($r:expr) => { $r.or_else(|_| Ok(())) }; }",
        ):
            with (
                self.subTest(source=source),
                self.assertRaisesRegex(ValueError, "opaque Rust macro"),
            ):
                scan(source)

    def test_macro_strings_and_assertions_are_not_blocked(self) -> None:
        source = """fn f(result: Result<(), ()>) {
    println!("if result.is_ok() { fallback(); } else { serve(); }");
    assert!(result.is_ok());
    check!{ if result.is_err() { return Err(()); } }
}
"""
        self.assertEqual(scan(source), [])

    def test_debug_divergence_is_ast_scoped(self) -> None:
        source = """// cfg!(debug_assertions)
const DOC: &str = "cfg!(debug_assertions)";
const RAW_DOC: &str = r#"
#[cfg(debug_assertions)]
"#;
#[cfg(debug_assertions)]
fn debug_only() {}
fn production() { if cfg!(debug_assertions) { fail_open(); } }
"""
        self.assertEqual(
            scan(source, include_debug=True),
            [(6, LINT.RULE_DEBUG_ASSERTIONS), (8, LINT.RULE_DEBUG_ASSERTIONS)],
        )
        self.assertEqual(scan(source), [])

    def test_debug_divergence_inside_opaque_macro_fails_closed(self) -> None:
        for source in (
            "macro_rules! debug_only { () => { cfg!(debug_assertions) }; }",
            "macro_rules! debug_only { () => { #[cfg(debug_assertions)] fn f() {} }; }",
        ):
            with (
                self.subTest(source=source),
                self.assertRaisesRegex(ValueError, "opaque Rust macro"),
            ):
                scan(source, include_debug=True)
        self.assertEqual(
            scan(
                "macro_rules! harmless { () => { let cfg = debug_assertions; }; }",
                include_debug=True,
            ),
            [],
        )

    def test_nested_debug_cfg_is_governed(self) -> None:
        source = """#[cfg(all(feature = "x", debug_assertions))]
fn debug_only() {}
fn f() { if cfg!(not(debug_assertions)) { diverge(); } }
#[cfg(feature = "debug_assertions")]
fn feature_only() {}
"""
        self.assertEqual(
            scan(source, include_debug=True),
            [(1, LINT.RULE_DEBUG_ASSERTIONS), (3, LINT.RULE_DEBUG_ASSERTIONS)],
        )

    def test_search_plane_authority_paths(self) -> None:
        source = """use ciborium as codec;
use git2::Repository as Repo;
extern crate tree_sitter;
use tree_sitter_language_pack as pack;
use std::process::Command as HostCommand;
use tokio::process::{Command, Stdio};
use std::{process::Command as StdCommand, io};
fn f(value: ciborium::value::Value) {
    let _ = ciborium::de::from_reader(input());
    let _ = git2::Repository::open(".");
    let _ = tree_sitter::Parser::new();
    let _ = tree_sitter_language_pack::get_parser("rust");
    let _ = Command::new("git");
    let _ = std::process::Command::new("git");
    let _ = tokio::process::Command::new("git");
}
"""
        self.assertEqual(
            scan(source, include_search_plane=True),
            [
                (1, LINT.RULE_CIBORIUM),
                (2, LINT.RULE_PRODUCER_PARSER),
                (3, LINT.RULE_PRODUCER_PARSER),
                (4, LINT.RULE_PRODUCER_PARSER),
                (5, LINT.RULE_PROCESS),
                (6, LINT.RULE_PROCESS),
                (7, LINT.RULE_PROCESS),
                (8, LINT.RULE_CIBORIUM),
                (9, LINT.RULE_CIBORIUM),
                (10, LINT.RULE_PRODUCER_PARSER),
                (11, LINT.RULE_PRODUCER_PARSER),
                (12, LINT.RULE_PRODUCER_PARSER),
                (13, LINT.RULE_PROCESS),
                (14, LINT.RULE_PROCESS),
                (15, LINT.RULE_PROCESS),
            ],
        )
        self.assertEqual(scan(source), [])

    def test_search_plane_authority_ignores_examples(self) -> None:
        source = """// use std::process::Command;
const DOC: &str = "git2::Repository ciborium::value::Value";
const RAW: &str = r#"
use tree_sitter::Parser;
tokio::process::Command::new("git");
"#;
fn process_id() { let _ = format!("{}", std::process::id()); }
"""
        self.assertEqual(scan(source, include_search_plane=True), [])

    def test_absolute_and_raw_search_plane_paths_are_governed(self) -> None:
        source = """fn f() {
    let _ = ::ciborium::de::from_reader(input());
    let _ = r#ciborium::de::from_reader(input());
    let _ = ::std::process::Command::new("git");
    let _ = Command::r#new("git");
}
"""
        self.assertEqual(
            scan(source, include_search_plane=True),
            [
                (2, LINT.RULE_CIBORIUM),
                (3, LINT.RULE_CIBORIUM),
                (4, LINT.RULE_PROCESS),
                (5, LINT.RULE_PROCESS),
            ],
        )

    def test_search_plane_macro_authority_fails_closed(self) -> None:
        for source in (
            'macro_rules! source { () => { git2::Repository::open(".") }; }',
            'fn f() { launch!{ Command::new("git") }; }',
        ):
            with (
                self.subTest(source=source),
                self.assertRaisesRegex(ValueError, "opaque Rust macro"),
            ):
                scan(source, include_search_plane=True)


if __name__ == "__main__":
    unittest.main()
