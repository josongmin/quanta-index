"""Counterexamples for the production Rust fallback syntax guard."""

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


def scan(source: str) -> list[tuple[int, str]]:
    return LINT.scan_source(source.encode(), PARSER)


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
}
"""
        self.assertEqual(
            scan(source),
            [(2, LINT.RULE_OR_ELSE), (3, LINT.RULE_OR_ELSE), (5, LINT.RULE_OR_ELSE)],
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


if __name__ == "__main__":
    unittest.main()
