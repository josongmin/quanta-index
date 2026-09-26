"""Module ratchets must reject empty and missing protected package selections."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "lint/check-cargo-modules-snapshot.py"


def test_empty_selection_cannot_be_a_passing_ratchet() -> None:
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--packages"], check=False, capture_output=True, text=True
    )
    assert result.returncode == 2
    assert "expected at least one argument" in result.stderr


def test_absent_protected_package_is_refused() -> None:
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--packages", "missing-guarded-package"],
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 1
    assert "protected crate is absent from workspace.members" in result.stderr


def test_missing_or_wrong_module_tree_root_is_refused(monkeypatch) -> None:
    import importlib.util

    import pytest

    spec = importlib.util.spec_from_file_location("modules_snapshot", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    for body in ("", "crate different_crate\n"):
        monkeypatch.setattr(
            module.subprocess,
            "run",
            lambda *args, body=body, **kwargs: subprocess.CompletedProcess([], 0, body, ""),
        )
        with pytest.raises(RuntimeError, match="missing or wrong crate root"):
            module.render_module_tree("quanta-index-core")


def test_canonical_tool_output_preserves_leading_blank_line(monkeypatch) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_snapshot", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    body = "\ncrate quanta_index_core\n"
    monkeypatch.setattr(
        module.subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess([], 0, body, ""),
    )
    assert module.render_module_tree("quanta-index-core") == body
