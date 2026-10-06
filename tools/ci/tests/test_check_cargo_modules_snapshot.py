"""Module ratchets must reject empty and missing protected package selections."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

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


def test_mixed_target_package_explicitly_selects_library(monkeypatch) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_mixed", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    seen = []

    def mixed_target_producer(command, **kwargs):
        seen.append(command)
        if "--lib" not in command:
            return subprocess.CompletedProcess(command, 1, "", "Multiple targets present")
        return subprocess.CompletedProcess(command, 0, "\ncrate mixed\n", "")

    monkeypatch.setattr(module.subprocess, "run", mixed_target_producer)
    assert module.render_module_tree("mixed") == "\ncrate mixed\n"
    assert seen == [["cargo", "modules", "structure", "--lib", "--package", "mixed", "--no-fns"]]


def test_explicit_library_selection_still_propagates_producer_failure(monkeypatch) -> None:
    import importlib.util

    import pytest

    spec = importlib.util.spec_from_file_location("modules_mixed", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    def failed_library_producer(command, **kwargs):
        assert "--lib" in command
        return subprocess.CompletedProcess(command, 1, "", "library analysis failed")

    monkeypatch.setattr(module.subprocess, "run", failed_library_producer)
    with pytest.raises(RuntimeError, match="library analysis failed"):
        module.render_module_tree("mixed")


@pytest.mark.parametrize("update", [False, True])
def test_unresolved_dependencies_cannot_compare_or_overwrite_baselines(
    tmp_path, monkeypatch, update
) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_unresolved", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    baseline = tmp_path / "quanta-index-contract.txt"
    baseline.write_text("crate quanta_index_contract\nexisting module\n")
    monkeypatch.setattr(module, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(module, "workspace_member_names", lambda: {"quanta-index-contract"})
    arguments = [str(SCRIPT), "--packages", "quanta-index-contract"]
    if update:
        arguments.append("--update-baseline")
    monkeypatch.setattr(sys, "argv", arguments)
    commands = []

    def unresolved(command, **kwargs):
        commands.append(command)
        assert command == [
            str(module.ROOT / "scripts" / "cargow"),
            "metadata",
            "--locked",
            "--format-version",
            "1",
        ]
        return subprocess.CompletedProcess(command, 101, "", "missing dependency feature")

    monkeypatch.setattr(module.subprocess, "run", unresolved)
    with pytest.raises(RuntimeError, match="missing dependency feature"):
        module.main()
    assert len(commands) == 1
    assert baseline.read_text() == "crate quanta_index_contract\nexisting module\n"


def test_resolved_dependencies_reach_the_module_comparison(tmp_path, monkeypatch) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_resolved", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    body = "crate quanta_index_contract\nexisting module\n"
    (tmp_path / "quanta-index-contract.txt").write_text(body)
    monkeypatch.setattr(module, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(module, "workspace_member_names", lambda: {"quanta-index-contract"})
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--packages", "quanta-index-contract"])
    commands = []

    def resolved(command, **kwargs):
        commands.append(command)
        return subprocess.CompletedProcess(
            command, 0, "{}" if command[1] == "metadata" else body, ""
        )

    monkeypatch.setattr(module.subprocess, "run", resolved)
    assert module.main() == 0
    assert commands[0][1:] == ["metadata", "--locked", "--format-version", "1"]
    assert commands[1] == [
        "cargo",
        "modules",
        "structure",
        "--lib",
        "--package",
        "quanta-index-contract",
        "--no-fns",
    ]
