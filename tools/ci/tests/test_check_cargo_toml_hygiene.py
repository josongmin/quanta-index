"""Tests for tools/ci/lint/check-cargo-toml-hygiene.py."""

from __future__ import annotations

import importlib.util
import sys
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "lint" / "check-cargo-toml-hygiene.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_cargo_toml_hygiene", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_cargo_toml_hygiene"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def _toml(text: str) -> dict:
    try:
        import tomllib  # type: ignore[unused-ignore]
    except ModuleNotFoundError:
        import tomli as tomllib  # type: ignore[no-redef]
    return tomllib.loads(textwrap.dedent(text))


def test_is_workspace_inherited_accepts_both_forms():
    assert MODULE.is_workspace_inherited(True) is True
    assert MODULE.is_workspace_inherited({"workspace": True}) is True
    assert MODULE.is_workspace_inherited({"workspace": False}) is False
    assert MODULE.is_workspace_inherited({"version": "1.0"}) is False
    assert MODULE.is_workspace_inherited("1.0") is False


def test_package_inherits_passes_dotted_form(tmp_path: Path):
    toml = _toml(
        """
        [package]
        name = "x"
        version.workspace = true
        edition.workspace = true
        license.workspace = true
        publish.workspace = true
        """
    )
    assert MODULE.check_package_inherits(tmp_path / "Cargo.toml", toml) == []


def test_package_inherits_fails_when_missing(tmp_path: Path):
    toml = _toml(
        """
        [package]
        name = "x"
        version = "0.1.0"
        edition.workspace = true
        license.workspace = true
        publish.workspace = true
        """
    )
    findings = MODULE.check_package_inherits(tmp_path / "Cargo.toml", toml)
    assert len(findings) == 1
    assert "version" in findings[0].message


def test_external_dep_must_use_workspace_form(tmp_path: Path):
    findings = MODULE.check_single_dep(
        tmp_path / "Cargo.toml", "dependencies", "serde", "1.0", internal=False
    )
    assert len(findings) == 1
    assert "bare version string" in findings[0].message


def test_external_dep_with_workspace_true_passes(tmp_path: Path):
    findings = MODULE.check_single_dep(
        tmp_path / "Cargo.toml",
        "dependencies",
        "serde",
        {"workspace": True, "features": ["derive"]},
        internal=False,
    )
    assert findings == []


def test_external_dep_with_inline_path_rejected(tmp_path: Path):
    findings = MODULE.check_single_dep(
        tmp_path / "Cargo.toml",
        "dependencies",
        "serde",
        {"path": "../vendored-serde"},
        internal=False,
    )
    messages = " | ".join(f.message for f in findings)
    assert "workspace = true" in messages
    assert "declares a path" in messages


def test_internal_dep_must_use_path_form(tmp_path: Path):
    findings = MODULE.check_single_dep(
        tmp_path / "Cargo.toml",
        "dependencies",
        "quanta-index-core",
        {"workspace": True},
        internal=True,
    )
    messages = " | ".join(f.message for f in findings)
    assert "must use path form" in messages


def test_internal_dep_with_path_passes(tmp_path: Path):
    crate_dir = REPO_ROOT / "crates" / "quanta-index-lexical"
    findings = MODULE.check_single_dep(
        crate_dir / "Cargo.toml",
        "dependencies",
        "quanta-index-core",
        {"version": "0.1.0", "path": "../quanta-index-core"},
        internal=True,
    )
    assert findings == []


def test_git_dep_is_rejected(tmp_path: Path):
    findings = MODULE.check_single_dep(
        tmp_path / "Cargo.toml",
        "dependencies",
        "weird",
        {"git": "https://example.com/repo"},
        internal=False,
    )
    messages = " | ".join(f.message for f in findings)
    assert "git source" in messages


def test_repo_audit_passes():
    """Real workspace must pass the hygiene gate."""
    violations: list = []
    for cargo_toml in MODULE.workspace_members():
        violations.extend(MODULE.audit_crate(cargo_toml))
    assert violations == [], "tree fails Cargo.toml hygiene:\n" + "\n".join(
        f"{v.path}: {v.message}" for v in violations
    )


def test_target_specific_dependency_tables_are_audited(tmp_path: Path):
    toml = _toml(
        """
        [dependencies]
        serde = { workspace = true }

        [target.'cfg(target_os = "macos")'.dependencies]
        nix = "0.31"
        """
    )
    findings = MODULE.check_dependencies(tmp_path / "Cargo.toml", toml)
    assert len(findings) == 1
    assert "nix" in findings[0].message
    assert "target." in findings[0].message
    assert "bare version string" in findings[0].message


def test_target_specific_workspace_dependency_passes(tmp_path: Path):
    toml = _toml(
        """
        [target.'cfg(target_os = "macos")'.dependencies]
        nix = { workspace = true }
        """
    )
    assert MODULE.check_dependencies(tmp_path / "Cargo.toml", toml) == []
