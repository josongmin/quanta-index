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
