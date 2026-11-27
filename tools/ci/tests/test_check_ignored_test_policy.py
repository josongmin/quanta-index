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


def _source_roots(root: Path) -> None:
    for relative in ("crates", "benchmarks"):
        (root / relative).mkdir(parents=True, exist_ok=True)
    (root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["shared/quanta-installed-artifact-trust"]\n'
        'exclude = ["shared/quanta-explicit-fd-process"]\n'
    )
    for package in ("quanta-installed-artifact-trust", "quanta-explicit-fd-process"):
        package_root = root / "shared" / package
        package_root.mkdir(parents=True)
        (package_root / "Cargo.toml").write_text(f'[package]\nname = "{package}"\n')


def test_bare_ignore_and_conditional_ignore_are_not_invisible(tmp_path: Path) -> None:
    _source_roots(tmp_path)
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
    _source_roots(tmp_path)
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
    _source_roots(tmp_path)
    path = tmp_path / "crates" / "demo.rs"
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


def test_missing_shared_member_cannot_certify_empty_policy(tmp_path: Path) -> None:
    (tmp_path / "crates").mkdir()
    (tmp_path / "benchmarks").mkdir()
    (tmp_path / "Cargo.toml").write_text('[workspace]\nmembers = ["shared/missing-trust"]\n')
    policy = tmp_path / "policy.toml"
    policy.write_text("exceptions = []\n")
    assert "missing shared workspace member" in _module().audit(tmp_path, policy)[0]


def test_workspace_shared_member_ignore_requires_exception(tmp_path: Path) -> None:
    _source_roots(tmp_path)
    source = tmp_path / "shared" / "quanta-installed-artifact-trust" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text('#[test]\n#[ignore = "manual"]\nfn shared_test() {}\n')
    policy = tmp_path / "policy.toml"
    policy.write_text("exceptions = []\n")

    assert (
        "unowned ignored test: shared/quanta-installed-artifact-trust/src/lib.rs::shared_test"
        in _module().audit(tmp_path, policy)
    )


def test_shared_inventory_includes_owned_standalone_and_excludes_build_output(
    tmp_path: Path,
) -> None:
    _source_roots(tmp_path)
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["shared/*"]\n'
        'exclude = ["shared/quanta-explicit-fd-process", "shared/unowned"]\n'
    )
    shared = tmp_path / "shared"
    standalone = shared / "quanta-explicit-fd-process"
    for package in (standalone, shared / "alternate-trust", shared / "unowned"):
        (package / "src").mkdir(parents=True)
        (package / "Cargo.toml").write_text('[package]\nname = "fixture"\n')
        (package / "src" / "lib.rs").write_text('#[test]\n#[ignore = "manual"]\nfn ignored() {}\n')
    generated = standalone / "target" / "generated.rs"
    generated.parent.mkdir()
    generated.write_text('#[test]\n#[ignore = "generated"]\nfn generated() {}\n')

    assert _module()._ignored_tests(tmp_path) == {
        ("shared/alternate-trust/src/lib.rs", "ignored", "manual"),
        ("shared/quanta-explicit-fd-process/src/lib.rs", "ignored", "manual"),
    }


def test_macro_ignore_cannot_silently_certify_policy(tmp_path: Path) -> None:
    _source_roots(tmp_path)
    source = tmp_path / "crates" / "demo.rs"
    source.write_text('macro_rules! m { () => { #[test] #[ignore = "slow"] fn f() {} }; }')
    policy = tmp_path / "policy.toml"
    policy.write_text("exceptions = []\n")
    assert "ignored test in opaque Rust macro" in _module().audit(tmp_path, policy)[0]


def test_one_exception_cannot_own_two_module_tests_with_same_short_name(tmp_path: Path) -> None:
    _source_roots(tmp_path)
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


def test_reason_drift_reports_source_and_policy_reasons_once(tmp_path: Path) -> None:
    _source_roots(tmp_path)
    source = tmp_path / "crates" / "demo.rs"
    source.write_text('#[test]\n#[ignore = "sealed revision"]\nfn large_view() {}\n')
    policy = tmp_path / "policy.toml"
    policy.write_text(
        '[[exceptions]]\npath = "crates/demo.rs"\ntest = "large_view"\n'
        'reason = "full publication"\nowner = "fixture"\ncadence = "weekly"\n'
        'review_by = "2099-01-01"\n'
    )

    assert _module().audit(tmp_path, policy) == [
        "ignored-test reason mismatch: crates/demo.rs::large_view "
        "(source: ['sealed revision'], policy: ['full publication'])"
    ]
