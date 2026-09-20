"""Regression tests for scripts/cargow lane-specific target-dir ownership."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT = REPO_ROOT / "scripts" / "cargow"
ENV_SCRIPT = REPO_ROOT / "scripts" / "quanta-index-env.sh"


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


def _source_env(env: dict[str, str], shell: str = "/bin/bash") -> list[str]:
    result = subprocess.run(
        [
            shell,
            "-c",
            'source "$1"; printf "%s\\n" "${RUSTC_WRAPPER:-}" "${SCCACHE_DIR:-}" '
            '"${SCCACHE_BASEDIRS:-}" "${SCCACHE_SERVER_PORT:-}"',
            "bash",
            str(ENV_SCRIPT),
        ],
        cwd=REPO_ROOT,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.splitlines()


def test_local_env_uses_repo_isolated_sccache_when_available(tmp_path: Path) -> None:
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    fake_sccache = fake_bin / "sccache"
    fake_sccache.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    fake_sccache.chmod(0o755)
    env = os.environ.copy()
    env["PATH"] = f"{fake_bin}:/usr/bin:/bin"
    env["QUANTA_INDEX_CACHE_ROOT"] = str(tmp_path / "cache")
    env["RUSTC_WRAPPER"] = "/another/repo/bin/sccache"
    env["QUANTA_INDEX_REPO_ROOT"] = "/another/repo"
    env["SCCACHE_DIR"] = "/another/repo/cache"
    env["SCCACHE_BASEDIRS"] = "/another/repo"
    env["SCCACHE_SERVER_PORT"] = "40001"
    env.pop("CI", None)

    for shell in ("/bin/bash", "/bin/zsh"):
        wrapper, cache_dir, base_dirs, port = _source_env(env, shell)

        assert wrapper == str(fake_sccache)
        assert cache_dir == str(tmp_path / "cache" / "sccache")
        assert base_dirs == str(REPO_ROOT)
        assert 40000 <= int(port) < 60000


def test_local_env_can_disable_sccache(tmp_path: Path) -> None:
    env = os.environ.copy()
    env["QUANTA_INDEX_CACHE_ROOT"] = str(tmp_path / "cache")
    env["QUANTA_INDEX_SCCACHE"] = "0"
    env["RUSTC_WRAPPER"] = "/another/repo/bin/sccache"
    env["SCCACHE_DIR"] = "/another/repo/cache"
    env["SCCACHE_BASEDIRS"] = "/another/repo"
    env["SCCACHE_SERVER_PORT"] = "40001"

    assert _source_env(env) == ["", "", "", ""]


def test_local_env_preserves_a_non_sccache_rustc_wrapper(tmp_path: Path) -> None:
    env = os.environ.copy()
    env["QUANTA_INDEX_CACHE_ROOT"] = str(tmp_path / "cache")
    env["RUSTC_WRAPPER"] = "/tools/custom-rustc-wrapper"
    env.pop("SCCACHE_DIR", None)
    env.pop("SCCACHE_BASEDIRS", None)
    env.pop("SCCACHE_SERVER_PORT", None)

    assert _source_env(env) == ["/tools/custom-rustc-wrapper", "", "", ""]
