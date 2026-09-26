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
