"""Public API baseline toolchain contract."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "lint/check-public-api.py"
sys.path.insert(0, str(SCRIPT.parent))  # Match Python's path when executing the checker directly.
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


def test_public_api_runner_selects_a_guarded_package_and_simplified_surface(monkeypatch) -> None:
    import subprocess

    calls = []

    def rendered(command, **kwargs):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, "pub mod quanta_index_contract\n", "")

    monkeypatch.setattr(MODULE.subprocess, "run", rendered)
    assert (
        MODULE.render_public_api("quanta-index-contract", "quanta_index_contract")
        == "pub mod quanta_index_contract\n"
    )
    command, kwargs = calls[0]
    assert command == [
        "cargo",
        "public-api",
        "--package",
        "quanta-index-contract",
        "--simplified",
    ]
    assert kwargs["cwd"] == MODULE.ROOT
    assert kwargs["env"]["RUSTUP_TOOLCHAIN"] == MODULE.PUBLIC_API_TOOLCHAIN


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
            MODULE.render_public_api("quanta-index-contract", "quanta_index_contract")


@pytest.mark.parametrize(
    ("target_kind", "target_name"),
    [("lib", "alias_library"), ("rlib", "snapshot_probe"), ("cdylib", "snapshot_probe")],
)
def test_public_api_uses_selected_library_target_for_snapshot_root(
    tmp_path, monkeypatch, target_kind, target_name
) -> None:
    monkeypatch.setattr(MODULE, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(
        sys, "argv", [str(SCRIPT), "--update-baseline", "--packages", "snapshot-probe"]
    )
    monkeypatch.setattr(
        MODULE,
        "validate_workspace_dependencies",
        lambda: {
            "snapshot-probe": {"targets": [{"kind": [target_kind], "name": target_name}]},
            "bin-only": {"targets": [{"kind": ["bin"], "name": "bin_only"}]},
        },
        raising=False,
    )
    commands = []
    expected = f"pub mod {target_name}\n"

    def producer(command, **kwargs):
        commands.append(command)
        return subprocess.CompletedProcess(command, 0, expected, "")

    monkeypatch.setattr(MODULE.subprocess, "run", producer)
    assert MODULE.main() == 0
    assert commands == [["cargo", "public-api", "--package", "snapshot-probe", "--simplified"]]
    assert (tmp_path / "snapshot-probe.txt").read_text() == expected

    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--update-baseline", "--packages", "bin-only"])
    with pytest.raises(RuntimeError, match="no unique library target"):
        MODULE.main()
    assert len(commands) == 1


def test_missing_member_cannot_update_other_public_api_baselines(tmp_path, monkeypatch) -> None:
    existing = tmp_path / "quanta-index-contract.txt"
    existing.write_text("previous\n")
    monkeypatch.setattr(MODULE, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(
        sys,
        "argv",
        [str(SCRIPT), "--update-baseline", "--packages", "quanta-index-contract", "missing"],
    )
    monkeypatch.setattr(
        MODULE,
        "validate_workspace_dependencies",
        lambda: {
            "quanta-index-contract": {
                "targets": [{"kind": ["lib"], "name": "quanta_index_contract"}]
            }
        },
        raising=False,
    )
    monkeypatch.setattr(MODULE, "render_public_api", lambda *_: pytest.fail("producer ran"))
    assert MODULE.main() == 1
    assert existing.read_text() == "previous\n"


def test_locked_metadata_target_selects_public_api_root(tmp_path, monkeypatch) -> None:
    body = "pub mod alias_library\n"
    (tmp_path / "snapshot-probe.txt").write_text(body)
    monkeypatch.setattr(MODULE, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--packages", "snapshot-probe"])
    commands = []

    def producer(command, **kwargs):
        commands.append(command)
        if command[1] == "metadata":
            return subprocess.CompletedProcess(
                command,
                0,
                '{"packages":[{"id":"probe","name":"snapshot-probe",'
                '"targets":[{"kind":["lib"],"name":"alias_library"}]}],'
                '"workspace_members":["probe"],'
                '"resolve":{"nodes":[{"id":"probe","dependencies":[]}]}}',
                "",
            )
        return subprocess.CompletedProcess(command, 0, body, "")

    monkeypatch.setattr(MODULE.subprocess, "run", producer)
    assert MODULE.main() == 0
    assert commands[0] == [
        str(MODULE.ROOT / "scripts" / "cargow"),
        "metadata",
        "--locked",
        "--format-version",
        "1",
    ]
    assert commands[1] == ["cargo", "public-api", "--package", "snapshot-probe", "--simplified"]


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
    monkeypatch.setattr(
        MODULE,
        "validate_workspace_dependencies",
        lambda: {
            name: {"targets": [{"kind": ["lib"], "name": name.replace("-", "_")}]}
            for name in MODULE.GUARDED_CRATES
        },
    )

    def render(package: str, _library_name: str) -> str:
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
        "validate_workspace_dependencies",
        lambda: {
            name: {"targets": [{"kind": ["lib"], "name": name.replace("-", "_")}]}
            for name in MODULE.GUARDED_CRATES
        },
    )
    monkeypatch.setattr(
        MODULE,
        "render_public_api",
        lambda package, _library_name: contract if package == "quanta-index-contract" else sdk,
    )
    assert MODULE.main() == 0
    assert (tmp_path / "quanta-index-contract.txt").read_text() == contract
    assert (tmp_path / "quanta-index-sdk.txt").read_text() == sdk
