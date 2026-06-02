"""Regression tests for scripts/cargow lane-specific target-dir ownership."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT = REPO_ROOT / "scripts" / "cargow"


def _run_metadata(*args: str, env: dict[str, str]) -> dict[str, object]:
    result = subprocess.run(
        [str(SCRIPT), *args, "metadata", "--format-version", "1", "--no-deps"],
        cwd=REPO_ROOT,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def test_explicit_lane_overrides_inherited_target_dir(tmp_path: Path) -> None:
    env = os.environ.copy()
    env["QUANTA_INDEX_BUILD_LOGGING"] = "0"
    env["CARGO_TARGET_DIR"] = str(tmp_path / "stale-target")
    payload = _run_metadata("--lane", "release-bin-lane", env=env)
    assert str(payload["target_directory"]).endswith("/target/release-bin-lane")


def test_preserve_opt_out_keeps_inherited_target_dir(tmp_path: Path) -> None:
    inherited = tmp_path / "custom-target"
    env = os.environ.copy()
    env["QUANTA_INDEX_BUILD_LOGGING"] = "0"
    env["QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR"] = "1"
    env["CARGO_TARGET_DIR"] = str(inherited)
    payload = _run_metadata("--lane", "release-bin-lane", env=env)
    assert str(payload["target_directory"]) == str(inherited)
