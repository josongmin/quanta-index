"""Tests for tools/ci/lint/check-error-shape.py."""

from __future__ import annotations

import importlib.util
import sys
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-error-shape.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_error_shape", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_error_shape"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def write(tmp_path: Path, content: str) -> Path:
    p = tmp_path / "err.rs"
    p.write_text(textwrap.dedent(content), encoding="utf-8")
    return p


def test_well_formed_error_enum_passes(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, Error)]
        pub enum FooError {
            #[error("a")]
            A,
            #[error("b")]
            B(String),
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_qualified_thiserror_derive_recognized(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, thiserror::Error)]
        pub enum BarError {
            #[error("a")]
            A,
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_missing_derive_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug)]
        pub enum NoDeriveError {
            A,
        }
        """,
    )
    findings = MODULE.audit_file(p)
    messages = [v.message for v in findings]
    assert any("missing a `#[derive(... Error" in m for m in messages)


def test_variant_missing_error_attr_is_flagged(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, Error)]
        pub enum HoleError {
            #[error("ok")]
            Ok,
            BareVariant,
        }
        """,
    )
    findings = MODULE.audit_file(p)
    assert len(findings) == 1
    assert "BareVariant" in findings[0].message


def test_struct_error_needs_derive(tmp_path: Path):
    p = write(
        tmp_path,
        """
        #[derive(Debug, Error)]
        #[error("plain")]
        pub struct PlainError {
            message: String,
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_non_error_enums_are_ignored(tmp_path: Path):
    p = write(
        tmp_path,
        """
        pub enum Mood {
            Happy,
            Sad,
        }
        """,
    )
    assert MODULE.audit_file(p) == []


def test_repo_audit_passes():
    """Existing *Error types must already satisfy the shape rules."""
    violations: list = []
    for path in MODULE.crate_source_files():
        violations.extend(MODULE.audit_file(path))
    assert violations == [], "tree has error-shape violations:\n" + "\n".join(
        f"{v.path}:{v.line}: {v.message}" for v in violations
    )
