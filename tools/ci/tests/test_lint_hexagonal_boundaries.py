from __future__ import annotations

import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
LINT = ROOT / "tools" / "ci" / "lint" / "lint-hexagonal-boundaries.py"


def test_hexagonal_boundary_lint_passes_on_repo() -> None:
    completed = subprocess.run(
        [sys.executable, str(LINT)],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 0, completed.stderr or completed.stdout
