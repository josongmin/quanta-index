"""Tests for tools/ci/lint/check-module-discipline.py."""

from __future__ import annotations

import importlib.util
import sys
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-module-discipline.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_module_discipline", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_module_discipline"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def write(tmp_path: Path, content: str) -> Path:
    p = tmp_path / "mod.rs"
    p.write_text(textwrap.dedent(content), encoding="utf-8")
    return p


def test_pure_facade_passes(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #![forbid(unsafe_code)]
        //! a doc comment
        // a line comment

        pub mod foo;
        pub mod bar;
        mod private;

        pub use foo::Thing;
        pub use bar::{
            One,
            Two,
        };
        """,
    )
    assert MODULE.audit_facade(p) == []


def test_inline_fn_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub mod foo;

        pub fn helper() -> i32 { 0 }
        """,
    )
    findings = MODULE.audit_facade(p)
    assert len(findings) == 1
    assert "pub fn helper" in findings[0].snippet


def test_inline_struct_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub mod foo;

        pub struct Sneaky {
            value: i32,
        }
        """,
    )
    findings = MODULE.audit_facade(p)
    # The struct opening line is flagged; the body lines are inside an open
    # brace so they're treated as continuation.
    assert any("pub struct Sneaky" in v.snippet for v in findings)


def test_inline_mod_block_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub mod foo;

        mod inline_tests {
            // ...
        }
        """,
    )
    findings = MODULE.audit_facade(p)
    assert any("mod inline_tests" in v.snippet for v in findings)


def test_pub_const_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub mod foo;

        pub const VERSION: u32 = 1;
        """,
    )
    findings = MODULE.audit_facade(p)
    assert any("pub const" in v.snippet for v in findings)


def test_impl_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub mod foo;

        impl Display for foo::Thing {}
        """,
    )
    findings = MODULE.audit_facade(p)
    assert any("impl" in v.snippet for v in findings)


def test_multiline_pub_use_tree_is_clean(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub mod foo;

        pub use foo::{
            Alpha,
            Beta,
            Gamma,
        };
        """,
    )
    assert MODULE.audit_facade(p) == []


def test_attributes_allowed(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #![allow(clippy::module_inception)]
        #[cfg(feature = "x")]
        pub mod gated;
        """,
    )
    assert MODULE.audit_facade(p) == []


def test_macro_rules_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub mod foo;

        macro_rules! sneaky { () => {} }
        """,
    )
    findings = MODULE.audit_facade(p)
    assert any("macro_rules!" in v.snippet for v in findings)


def test_lint_loads_workspace_members():
    """The lint correctly reads workspace.members and produces a non-empty
    facade-file list. Intentionally not asserting the tree itself passes —
    the lint is expected to surface existing structural debt until the
    tree is cleaned up. Pre-commit and CI run the lint against the tree and
    will block PRs until that debt is resolved."""
    facade_files = MODULE.collect_facade_files()
    assert len(facade_files) > 0, "lint sees no facade files — workspace.members empty?"
