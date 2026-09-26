"""Ignored tests must be inventoried even without a reason attribute."""

from __future__ import annotations

import importlib.util
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "lint" / "check-ignored-test-policy.py"


def _module():
    spec = importlib.util.spec_from_file_location("check_ignored_test_policy", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_bare_ignore_and_conditional_ignore_are_not_invisible(tmp_path: Path) -> None:
    (tmp_path / "benchmarks").mkdir()
    path = tmp_path / "crates" / "demo" / "src" / "lib.rs"
    path.parent.mkdir(parents=True)
    path.write_text(
        "#[test]\n#[ignore]\nfn bare() {}\n"
        '#[test]\n#[cfg_attr(miri, ignore = "miri-specific")]\nfn conditional() {}\n'
    )
    assert _module()._ignored_tests(tmp_path) == {
        ("crates/demo/src/lib.rs", "bare", ""),
        ("crates/demo/src/lib.rs", "conditional", "miri-specific"),
    }


def test_formatted_nested_conditional_ignore_is_visible(tmp_path: Path) -> None:
    (tmp_path / "crates").mkdir()
    path = tmp_path / "benchmarks" / "demo" / "src" / "lib.rs"
    path.parent.mkdir(parents=True)
    path.write_text("""#[test]
        #[ cfg_attr (
            all(), cfg_attr ( all(), ignore = r#"conditional"# )
        ) ]
        fn conditional() {}
    """)
    assert _module()._ignored_tests(tmp_path) == {
        ("benchmarks/demo/src/lib.rs", "conditional", "conditional"),
    }


def test_all_conditional_reasons_are_inventoried(tmp_path: Path) -> None:
    (tmp_path / "benchmarks").mkdir()
    path = tmp_path / "crates" / "demo.rs"
    path.parent.mkdir()
    path.write_text("""#[test]
        #[cfg_attr(all(), ignore = "first", cfg_attr(all(), ignore = "second"))]
        fn both() {}
        #[cfg_attr(ignore, test)]
        fn predicate_only() {}
    """)
    assert _module()._ignored_tests(tmp_path) == {
        ("crates/demo.rs", "both", "first"),
        ("crates/demo.rs", "both", "second"),
    }


def test_missing_source_roots_cannot_certify_empty_policy(tmp_path: Path) -> None:
    policy = tmp_path / "policy.toml"
    policy.write_text("exceptions = []\n")
    assert "Rust source roots are missing" in _module().audit(tmp_path, policy)[0]


def test_macro_ignore_cannot_silently_certify_policy(tmp_path: Path) -> None:
    (tmp_path / "benchmarks").mkdir()
    source = tmp_path / "crates" / "demo.rs"
    source.parent.mkdir()
    source.write_text('macro_rules! m { () => { #[test] #[ignore = "slow"] fn f() {} }; }')
    policy = tmp_path / "policy.toml"
    policy.write_text("exceptions = []\n")
    assert "ignored test in opaque Rust macro" in _module().audit(tmp_path, policy)[0]


def test_one_exception_cannot_own_two_module_tests_with_same_short_name(tmp_path: Path) -> None:
    (tmp_path / "crates").mkdir()
    (tmp_path / "benchmarks").mkdir()
    source = tmp_path / "crates" / "demo.rs"
    source.write_text(
        'mod first { #[test] #[ignore = "slow"] fn same() {} }\n'
        'mod second { #[test] #[ignore = "slow"] fn same() {} }\n'
    )
    policy = tmp_path / "policy.toml"
    policy.write_text(
        '[[exceptions]]\npath = "crates/demo.rs"\ntest = "same"\n'
        'reason = "slow"\nowner = "fixture"\ncadence = "weekly"\n'
        'review_by = "2099-01-01"\n'
    )
    assert "ambiguous ignored-test function identity" in _module().audit(tmp_path, policy)[0]
