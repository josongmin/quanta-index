"""Public API baseline toolchain contract."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "lint/check-public-api.py"
SPEC = importlib.util.spec_from_file_location("check_public_api", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def test_public_api_runner_uses_the_baseline_nightly(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setenv("RUSTUP_TOOLCHAIN", "nightly")
    monkeypatch.setenv("QUANTA_INDEX_CACHE_ROOT", str(tmp_path))
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)

    env = MODULE.cargo_env("public-api-lane")

    assert env["RUSTUP_TOOLCHAIN"] == "nightly-2026-08-01"
    assert env["CARGO_TARGET_DIR"].startswith(str(tmp_path / "target"))


def test_empty_package_selection_is_refused(monkeypatch) -> None:
    import pytest

    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--packages"])
    with pytest.raises(SystemExit) as error:
        MODULE.parse_args()
    assert error.value.code == 2


def test_public_api_empty_or_wrong_tool_output_is_refused(monkeypatch) -> None:
    import subprocess

    import pytest

    for body in ("", "pub mod different_crate\n"):
        monkeypatch.setattr(
            MODULE.subprocess,
            "run",
            lambda *args, body=body, **kwargs: subprocess.CompletedProcess([], 0, body, ""),
        )
        with pytest.raises(RuntimeError, match="missing or wrong crate root"):
            MODULE.render_public_api("quanta-index-contract")


def test_update_keeps_both_api_baselines_when_second_producer_fails(
    monkeypatch, tmp_path: Path
) -> None:
    import pytest

    contract = tmp_path / "quanta-index-contract.txt"
    sdk = tmp_path / "quanta-index-sdk.txt"
    contract.write_text("contract previous\n")
    sdk.write_text("sdk previous\n")
    monkeypatch.setattr(MODULE, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--update-baseline"])

    def render(package: str) -> str:
        if package == "quanta-index-sdk":
            raise RuntimeError("second API render failed")
        return "pub mod quanta_index_contract\nnew export\n"

    monkeypatch.setattr(MODULE, "render_public_api", render)
    with pytest.raises(RuntimeError, match="second API render failed"):
        MODULE.main()
    assert contract.read_text() == "contract previous\n"
    assert sdk.read_text() == "sdk previous\n"


def test_update_writes_all_rendered_api_baselines(monkeypatch, tmp_path: Path) -> None:
    contract = "pub mod quanta_index_contract\nnew export\n"
    sdk = "pub mod quanta_index_sdk\nnew export\n"
    monkeypatch.setattr(MODULE, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--update-baseline"])
    monkeypatch.setattr(
        MODULE,
        "render_public_api",
        lambda package: contract if package == "quanta-index-contract" else sdk,
    )
    assert MODULE.main() == 0
    assert (tmp_path / "quanta-index-contract.txt").read_text() == contract
    assert (tmp_path / "quanta-index-sdk.txt").read_text() == sdk
