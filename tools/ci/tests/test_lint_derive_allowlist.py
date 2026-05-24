"""Tests for tools/ci/lint/check-rust-derive-allowlist.py."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-rust-derive-allowlist.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_derive_allowlist", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_derive_allowlist"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def test_allowlist_passes_for_cheap_derives(tmp_path: Path):
    f = tmp_path / "ok.rs"
    f.write_text(
        "#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Ord, PartialOrd)]\n"
        "struct A;\n"
    )
    assert MODULE.audit_file(f) == []


def test_thiserror_error_is_allowed(tmp_path: Path):
    f = tmp_path / "err.rs"
    f.write_text("#[derive(Debug, Error)]\nenum E { A }\n")
    assert MODULE.audit_file(f) == []


def test_qualified_thiserror_error_is_allowed(tmp_path: Path):
    f = tmp_path / "err_q.rs"
    f.write_text("#[derive(Debug, thiserror::Error)]\nenum E { A }\n")
    assert MODULE.audit_file(f) == []


def test_serialize_derive_is_flagged(tmp_path: Path):
    f = tmp_path / "bad_serde.rs"
    f.write_text("#[derive(Serialize)]\nstruct X;\n")
    findings = MODULE.audit_file(f)
    assert len(findings) == 1
    assert "Serialize" in findings[0]
    assert "banned outright" in findings[0]


def test_qualified_serde_serialize_is_flagged(tmp_path: Path):
    f = tmp_path / "bad_serde_q.rs"
    f.write_text("#[derive(serde::Serialize, serde::Deserialize)]\nstruct X;\n")
    findings = MODULE.audit_file(f)
    assert len(findings) == 2


def test_unknown_derive_is_flagged(tmp_path: Path):
    f = tmp_path / "unknown.rs"
    f.write_text("#[derive(strum::EnumIter)]\nenum X { A }\n")
    findings = MODULE.audit_file(f)
    assert len(findings) == 1
    assert "not on the allowlist" in findings[0]
    assert "EnumIter" in findings[0]


def test_multiline_derive_parses_each_entry(tmp_path: Path):
    f = tmp_path / "multi.rs"
    f.write_text(
        "#[derive(\n"
        "  Debug,\n"
        "  Clone,\n"
        "  EnumIter\n"
        ")]\n"
        "struct Z;\n"
    )
    findings = MODULE.audit_file(f)
    assert len(findings) == 1
    assert "EnumIter" in findings[0]


def test_repo_audit_passes():
    """Whole-tree sanity: existing crates must satisfy the allowlist."""
    findings: list[str] = []
    for rs in sorted((REPO_ROOT / "crates").rglob("*.rs")):
        if "/target/" in str(rs):
            continue
        findings.extend(MODULE.audit_file(rs))
    assert findings == [], "existing tree violates derive allowlist:\n" + "\n".join(
        findings
    )


def test_clippy_attribute_is_not_a_derive(tmp_path: Path):
    """Defensive: `#[derive(...)]` regex must not catch other attributes."""
    f = tmp_path / "attrs.rs"
    f.write_text("#[allow(clippy::derive_partial_eq_without_eq)]\nstruct Y;\n")
    assert MODULE.audit_file(f) == []


def test_main_returns_zero_on_clean_repo():
    """End-to-end smoke: running main() against the real tree returns 0."""
    rc = MODULE.main()
    assert rc == 0
