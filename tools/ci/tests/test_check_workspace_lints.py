"""Workspace-member coverage for the duplicate-lint policy guard."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "scripts" / "check_workspace_lints.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_workspace_lints", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_workspace_lints"] = module
    spec.loader.exec_module(module)
    return module


def test_guard_covers_non_crates_workspace_member(tmp_path: Path, monkeypatch, capsys):
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/core", "benchmarks/retrieval"]\n',
        encoding="utf-8",
    )
    for member in ("crates/core", "benchmarks/retrieval"):
        member_dir = tmp_path / member
        (member_dir / "src").mkdir(parents=True)
        (member_dir / "Cargo.toml").write_text("[lints]\nworkspace = true\n", encoding="utf-8")
    (tmp_path / "benchmarks/retrieval/src/lib.rs").write_text(
        "#![expect(clippy::multiple_crate_versions)]\n", encoding="utf-8"
    )
    module = _load_module()
    monkeypatch.setattr(module, "ROOT", tmp_path)
    assert module.main() == 1
    assert "benchmarks/retrieval/src/lib.rs" in capsys.readouterr().out
