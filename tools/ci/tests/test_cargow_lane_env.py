"""Regression tests for scripts/cargow lane-specific target-dir ownership."""

from __future__ import annotations

import hashlib
import json
import os
import runpy
import shutil
import subprocess
from pathlib import Path

import pytest

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
    checkout_id = hashlib.sha256(str(REPO_ROOT.resolve()).encode()).hexdigest()[:16]
    assert str(payload["target_directory"]).endswith(
        f"/target/{checkout_id}/release-bin-lane"
    )


def test_distinct_checkouts_cannot_share_a_cargo_target_lane(tmp_path: Path) -> None:
    cache_root = tmp_path / "cache"
    env = os.environ.copy()
    env["QUANTA_INDEX_CACHE_ROOT"] = str(cache_root)
    env["QUANTA_INDEX_BUILD_LANE"] = "clippy-lane"
    env["QUANTA_INDEX_SCCACHE"] = "0"
    env.pop("CARGO_TARGET_DIR", None)
    targets: list[str] = []

    for name in ("checkout-a", "checkout-b"):
        scripts = tmp_path / name / "scripts"
        scripts.mkdir(parents=True)
        copied_env = scripts / "quanta-index-env.sh"
        shutil.copyfile(ENV_SCRIPT, copied_env)
        result = subprocess.run(
            [
                "/bin/bash",
                "-c",
                'source "$1"; printf "%s" "$CARGO_TARGET_DIR"',
                "bash",
                str(copied_env),
            ],
            cwd=tmp_path,
            env=env,
            check=True,
            capture_output=True,
            text=True,
        )
        targets.append(result.stdout)

    assert targets[0] != targets[1]
    assert all(target.startswith(str(cache_root / "target")) for target in targets)
    assert all(target.endswith("/clippy-lane") for target in targets)


@pytest.mark.parametrize(
    "script_name",
    ["check-public-api.py", "check-cargo-modules-snapshot.py"],
)
def test_auxiliary_cargo_tools_share_the_checkout_namespace(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, script_name: str
) -> None:
    cache_root = tmp_path / "cache"
    monkeypatch.setenv("QUANTA_INDEX_CACHE_ROOT", str(cache_root))
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    cargo_env = runpy.run_path(str(REPO_ROOT / "tools" / "ci" / "lint" / script_name))[
        "cargo_env"
    ]
    checkout_id = hashlib.sha256(str(REPO_ROOT.resolve()).encode()).hexdigest()[:16]
    lane = "auxiliary-lane"
    assert cargo_env(lane)["CARGO_TARGET_DIR"] == str(
        cache_root / "target" / checkout_id / lane
    )


def test_preserve_opt_out_keeps_inherited_target_dir(tmp_path: Path) -> None:
    inherited = tmp_path / "custom-target"
    env = os.environ.copy()
    env["QUANTA_INDEX_BUILD_LOGGING"] = "0"
    env["QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR"] = "1"
    env["CARGO_TARGET_DIR"] = str(inherited)
    payload = _run_metadata("--lane", "release-bin-lane", env=env)
    assert str(payload["target_directory"]) == str(inherited)


@pytest.mark.parametrize(
    ("args", "inherited_lane"),
    [
        (("--lane", "../escaped"), None),
        ((), "../escaped"),
        (("--lane", ""), None),
        (("--lane", "UPPER-lane"), None),
    ],
)
def test_invalid_lane_is_rejected_before_cargo(
    tmp_path: Path, args: tuple[str, ...], inherited_lane: str | None
) -> None:
    env = os.environ.copy()
    env["QUANTA_INDEX_BUILD_LOGGING"] = "0"
    env["QUANTA_INDEX_CACHE_ROOT"] = str(tmp_path / "cache")
    if inherited_lane is None:
        env.pop("QUANTA_INDEX_BUILD_LANE", None)
    else:
        env["QUANTA_INDEX_BUILD_LANE"] = inherited_lane

    result = subprocess.run(
        [str(SCRIPT), *args, "metadata", "--format-version", "1", "--no-deps"],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )

    assert result.returncode == 2
    assert "invalid cargo lane" in result.stderr
    assert not (tmp_path / "cache" / "escaped").exists()


def test_workspace_nextest_builds_and_exports_explicit_searchd_pin(tmp_path: Path) -> None:
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    log = tmp_path / "cargo.log"
    fake_cargo = fake_bin / "cargo"
    fake_cargo.write_text(
        "#!/bin/bash\n"
        "set -euo pipefail\n"
        "printf '%s|%s\\n' \"${QUANTA_INDEX_SEARCHD_BIN:-}\" \"$*\" >> \"$CARGO_CALL_LOG\"\n"
        "if [[ \"${1:-}\" == build ]]; then\n"
        "  mkdir -p \"$CARGO_TARGET_DIR/debug\"\n"
        "  printf '#!/bin/sh\\nexit 0\\n' > \"$CARGO_TARGET_DIR/debug/quanta-index-searchd\"\n"
        "  chmod +x \"$CARGO_TARGET_DIR/debug/quanta-index-searchd\"\n"
        "fi\n",
        encoding="utf-8",
    )
    fake_cargo.chmod(0o755)
    env = os.environ.copy()
    env["PATH"] = f"{fake_bin}:{env['PATH']}"
    env["CARGO_CALL_LOG"] = str(log)
    env["QUANTA_INDEX_BUILD_LOGGING"] = "0"
    env["QUANTA_INDEX_CACHE_ROOT"] = str(tmp_path / "cache")
    env["QUANTA_INDEX_SCCACHE"] = "0"
    env.pop("QUANTA_INDEX_SEARCHD_BIN", None)

    result = subprocess.run(
        [str(SCRIPT), "--lane", "test-workspace-lane", "nextest", "run", "--workspace"],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )

    assert result.returncode == 0, result.stderr
    calls = log.read_text(encoding="utf-8").splitlines()
    assert calls[0].endswith(
        "|build -p quanta-index-searchd-runtime --bin quanta-index-searchd --locked"
    )
    pin, command = calls[1].split("|", 1)
    pin_path = Path(pin)
    assert pin_path.name == "quanta-index-searchd"
    assert pin_path.parent.name == "debug"
    assert pin_path.parent.parent.name == "test-workspace-lane"
    assert command == "nextest run --workspace"


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
