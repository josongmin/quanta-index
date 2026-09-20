"""Tests for the single-process local nextest scope runner."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "run-local-test-scope.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("run_local_test_scope", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["run_local_test_scope"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def _catalog(tmp_path: Path) -> dict:
    first = tmp_path / "crates" / "first" / "tests" / "alpha.rs"
    second = tmp_path / "crates" / "second" / "tests" / "beta.rs"
    first.parent.mkdir(parents=True)
    second.parent.mkdir(parents=True)
    first.write_text("", encoding="utf-8")
    second.write_text("", encoding="utf-8")
    return {
        "local_scopes": {
            "one": {"lane": "one-lane", "test_threads": 2, "targets": ["first-alpha"]},
            "all-second": {"lane": "two-lane", "test_threads": 4, "owners": ["second"]},
        },
        "integration_targets": [
            {
                "id": "first-alpha",
                "owner": "first",
                "path": str(first.relative_to(tmp_path)),
            },
            {
                "id": "second-beta",
                "owner": "second",
                "path": str(second.relative_to(tmp_path)),
            },
        ],
    }


def test_combined_scopes_build_one_nextest_command(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    lane, test_threads, include_lib, extra_packages, targets = MODULE.resolve_targets(
        _catalog(tmp_path), ["one", "all-second"]
    )

    assert lane == "local-validation-lane"
    assert test_threads == 2
    assert include_lib is False
    assert extra_packages == []
    assert [target["id"] for target in targets] == ["first-alpha", "second-beta"]
    command = MODULE.build_command(lane, test_threads, include_lib, extra_packages, targets)
    assert command.count("nextest") == 1
    assert command.count("run") == 1
    assert command.count("-p") == 2
    assert command.count("--test") == 2
    assert command[command.index("--status-level") + 1] == "fail"
    assert command[command.index("--final-status-level") + 1] == "fail"


def test_scope_refuses_cross_product_target_leak(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    shadow = tmp_path / "crates" / "second" / "tests" / "alpha.rs"
    shadow.write_text("", encoding="utf-8")
    data["integration_targets"].append(
        {"id": "second-alpha", "owner": "second", "path": str(shadow.relative_to(tmp_path))}
    )
    data["local_scopes"]["mixed"] = {
        "lane": "mixed-lane",
        "test_threads": 2,
        "targets": ["first-alpha", "second-beta"],
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    try:
        MODULE.resolve_targets(data, ["mixed"])
    except ValueError as error:
        assert "undeclared package/target pair" in str(error)
    else:  # pragma: no cover - regression assertion
        raise AssertionError("cross-product target leak was accepted")


def test_library_and_integration_selectors_are_rejected(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    library = tmp_path / "crates" / "library"
    library.mkdir(parents=True)
    (library / "Cargo.toml").write_text(
        '[package]\nname = "library"\nversion = "0.1.0"\n', encoding="utf-8"
    )
    data["local_scopes"]["composite"] = {
        "lane": "composite-lane",
        "test_threads": 3,
        "includes": ["one"],
        "packages": ["library"],
        "lib": True,
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    try:
        MODULE.resolve_targets(data, ["composite"])
    except ValueError as error:
        assert "library and integration selectors" in str(error)
    else:  # pragma: no cover - regression assertion
        raise AssertionError("mixed library/integration selectors were accepted")


def test_library_only_scope_builds_one_nextest_command(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    library = tmp_path / "crates" / "library"
    library.mkdir(parents=True)
    (library / "Cargo.toml").write_text(
        '[package]\nname = "library"\nversion = "0.1.0"\n', encoding="utf-8"
    )
    data["local_scopes"]["library"] = {
        "lane": "library-lane",
        "test_threads": 3,
        "packages": ["library"],
        "lib": True,
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    lane, threads, include_lib, packages, targets = MODULE.resolve_targets(data, ["library"])
    command = MODULE.build_command(lane, threads, include_lib, packages, targets)

    assert targets == []
    assert include_lib is True
    assert packages == ["library"]
    assert "--lib" in command
    assert command.count("nextest") == 1


def test_include_cycle_is_rejected(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    data["local_scopes"]["one"]["includes"] = ["all-second"]
    data["local_scopes"]["all-second"]["includes"] = ["one"]
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    try:
        MODULE.resolve_targets(data, ["one"])
    except ValueError as error:
        assert "include cycle" in str(error)
    else:  # pragma: no cover - regression assertion
        raise AssertionError("scope include cycle was accepted")


def test_multiple_source_rows_can_share_one_cargo_test_target(tmp_path: Path, monkeypatch) -> None:
    data = _catalog(tmp_path)
    data["integration_targets"][0]["target"] = "grouped-suite"
    data["integration_targets"][1]["target"] = "grouped-suite"
    data["local_scopes"]["grouped"] = {
        "lane": "grouped-lane",
        "test_threads": 2,
        "targets": ["first-alpha", "second-beta"],
    }
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)

    lane, threads, include_lib, packages, targets = MODULE.resolve_targets(data, ["grouped"])
    command = MODULE.build_command(lane, threads, include_lib, packages, targets)

    assert command.count("--test") == 1
    assert command[command.index("--test") + 1] == "grouped-suite"
