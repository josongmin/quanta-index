"""Registry tests for the benchmark CLI authority mapping."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "benchmark" / "benchctl.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("benchctl", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["benchctl"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def test_manifest_maps_profiles_to_explicit_recipes_and_validator_profiles() -> None:
    profiles = MODULE.load_profiles()
    assert profiles["dsl-authority"]["artifact_profile"] == "dsl-authority"
    assert profiles["dsl-authority"]["recipes"] == ["rust-bench-dsl-warm", "rust-bench-dsl-cold"]
    assert profiles["dsl-authority"]["families"] == ["dsl-warm", "dsl-cold"]
    assert "rust-verify-quality-ann" in profiles["quality-full"]["recipes"]
    assert "rust-verify-quality-concurrency" in profiles["quality-full"]["recipes"]
    assert profiles["systems"]["families"] == ["freshness", "open-loop"]


def test_list_exposes_only_registered_profiles() -> None:
    result = subprocess.run(
        [sys.executable, str(SCRIPT_PATH), "list"],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    assert "dsl-authority\tdsl-authority" in result.stdout
    assert "quality-full\tquality-full" in result.stdout
