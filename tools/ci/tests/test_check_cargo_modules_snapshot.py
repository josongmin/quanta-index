"""Module ratchets must reject empty and missing protected package selections."""

from __future__ import annotations

import json
import os
import shlex
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "lint/check-cargo-modules-snapshot.py"
sys.path.insert(0, str(SCRIPT.parent))  # Match Python's path when executing the checker directly.


def _members(*names: str) -> dict[str, dict]:
    return {
        name: {"targets": [{"kind": ["lib"], "name": name.replace("-", "_")}]} for name in names
    }


def _record_child_check(kwargs: dict, status: str = "0\n") -> None:
    Path(kwargs["env"]["QUANTA_INDEX_MODULES_CHECK_STATUS"]).write_text(status)


def _stub_tool_lookup(module, monkeypatch) -> None:
    monkeypatch.setattr(module.shutil, "which", lambda executable, **_kwargs: f"/fake/{executable}")


def test_workspace_members_use_cargo_metadata_package_names() -> None:
    from workspace_metadata import workspace_members

    metadata = {
        "packages": [
            {
                "id": "path+file:///checkout/crates/moved-contract#0.1.0",
                "name": "quanta-index-contract",
            },
            {"id": "path+file:///checkout/crates/quanta-index-core#0.1.0", "name": "renamed-core"},
            {"id": "path+file:///checkout/crates/excluded#0.1.0", "name": "excluded"},
        ],
        "workspace_members": [
            "path+file:///checkout/crates/moved-contract#0.1.0",
            "path+file:///checkout/crates/quanta-index-core#0.1.0",
        ],
        "resolve": {
            "nodes": [
                {"id": "path+file:///checkout/crates/moved-contract#0.1.0", "dependencies": []},
                {"id": "path+file:///checkout/crates/quanta-index-core#0.1.0", "dependencies": []},
            ]
        },
    }
    assert set(workspace_members(metadata)) == {"quanta-index-contract", "renamed-core"}


@pytest.mark.parametrize(
    "metadata",
    [
        {},
        {"packages": [], "workspace_members": ["missing"]},
        {
            "packages": [{"id": "one", "name": "quanta-index-core"}],
            "workspace_members": ["missing"],
            "resolve": {"nodes": [{"id": "one", "dependencies": []}]},
        },
        {
            "packages": [{"id": "one", "name": "quanta-index-core"}],
            "workspace_members": ["one", "one"],
            "resolve": {"nodes": [{"id": "one", "dependencies": []}]},
        },
        {
            "packages": [{"id": "one", "name": "quanta-index-core"}],
            "workspace_members": ["one"],
            "resolve": {"nodes": [{"id": "other", "dependencies": []}]},
        },
        {
            "packages": [{"id": "one", "name": "quanta-index-core"}],
            "workspace_members": ["one"],
            "resolve": {"nodes": [{"id": "one", "dependencies": "not a list"}]},
        },
        {
            "packages": [{"id": "one", "name": "quanta-index-core"}],
            "workspace_members": ["one"],
            "resolve": {"nodes": [{"id": "one", "dependencies": ["missing"]}]},
        },
        pytest.param(
            {
                "packages": [
                    {"id": "one", "name": "quanta-index-core"},
                    {"id": "dependency", "name": "dependency"},
                ],
                "workspace_members": ["one"],
                "resolve": {"nodes": [{"id": "one", "dependencies": ["dependency"]}]},
            },
            id="dependency-package-without-resolve-node",
        ),
        {
            "packages": [{"id": "one", "name": "quanta-index-core"}],
            "workspace_members": ["one"],
            "resolve": {
                "nodes": [
                    {"id": "one", "dependencies": []},
                    {"id": "one", "dependencies": []},
                ]
            },
        },
    ],
)
def test_workspace_members_refuse_malformed_cargo_metadata(metadata) -> None:
    from workspace_metadata import workspace_members

    with pytest.raises(RuntimeError, match="cargo metadata"):
        workspace_members(metadata)


def test_empty_selection_cannot_be_a_passing_ratchet() -> None:
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--packages"], check=False, capture_output=True, text=True
    )
    assert result.returncode == 2
    assert "expected at least one argument" in result.stderr


def test_absent_protected_package_is_refused(monkeypatch, capsys) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_missing", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    monkeypatch.setattr(
        module, "validate_workspace_dependencies", lambda: _members("quanta-index-contract")
    )
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--packages", "missing-guarded-package"])

    assert module.main() == 1
    assert (
        "missing-guarded-package: protected crate is absent from workspace.members"
        in capsys.readouterr().err
    )


def test_missing_or_wrong_module_tree_root_is_refused(monkeypatch) -> None:
    import importlib.util

    import pytest

    spec = importlib.util.spec_from_file_location("modules_snapshot", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _stub_tool_lookup(module, monkeypatch)
    for body in ("", "crate different_crate\n"):

        def rendered(command, body=body, **kwargs):
            _record_child_check(kwargs)
            return subprocess.CompletedProcess(command, 0, body, "")

        monkeypatch.setattr(module.subprocess, "run", rendered)
        with pytest.raises(RuntimeError, match="missing or wrong crate root"):
            module.render_module_tree("quanta-index-core", "quanta_index_core")


def test_canonical_tool_output_preserves_leading_blank_line(monkeypatch) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_snapshot", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _stub_tool_lookup(module, monkeypatch)
    body = "\ncrate quanta_index_core\n"

    def rendered(command, **kwargs):
        _record_child_check(kwargs)
        return subprocess.CompletedProcess(command, 0, body, "")

    monkeypatch.setattr(module.subprocess, "run", rendered)
    assert module.render_module_tree("quanta-index-core", "quanta_index_core") == body


def test_mixed_target_package_explicitly_selects_library(monkeypatch) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_mixed", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _stub_tool_lookup(module, monkeypatch)
    seen = []

    def mixed_target_producer(command, **kwargs):
        seen.append(command)
        if "--lib" not in command:
            return subprocess.CompletedProcess(command, 1, "", "Multiple targets present")
        _record_child_check(kwargs)
        return subprocess.CompletedProcess(command, 0, "\ncrate mixed\n", "")

    monkeypatch.setattr(module.subprocess, "run", mixed_target_producer)
    assert module.render_module_tree("mixed", "mixed") == "\ncrate mixed\n"
    assert [Path(seen[0][0]).name, *seen[0][1:]] == [
        "cargo-modules",
        "modules",
        "structure",
        "--lib",
        "--package",
        "mixed",
        "--no-fns",
    ]


def test_library_alias_is_the_module_root_for_selected_package(tmp_path, monkeypatch) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_library_alias", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _stub_tool_lookup(module, monkeypatch)
    monkeypatch.setattr(module, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(
        module,
        "validate_workspace_dependencies",
        lambda: {
            "snapshot-probe": {"targets": [{"kind": ["lib"], "name": "alias_library"}]},
            "bin-only": {"targets": [{"kind": ["bin"], "name": "bin_only"}]},
        },
    )
    monkeypatch.setattr(
        sys, "argv", [str(SCRIPT), "--update-baseline", "--packages", "snapshot-probe"]
    )
    commands = []

    def producer(command, **kwargs):
        commands.append(command)
        _record_child_check(kwargs)
        return subprocess.CompletedProcess(command, 0, "\ncrate alias_library\n", "")

    monkeypatch.setattr(module.subprocess, "run", producer)
    assert module.main() == 0
    assert [[Path(command[0]).name, *command[1:]] for command in commands] == [
        [
            "cargo-modules",
            "modules",
            "structure",
            "--lib",
            "--package",
            "snapshot-probe",
            "--no-fns",
        ]
    ]
    assert (tmp_path / "snapshot-probe.txt").read_text() == "\ncrate alias_library\n"
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--update-baseline", "--packages", "bin-only"])
    with pytest.raises(RuntimeError, match="no unique library target"):
        module.main()
    assert len(commands) == 1


def test_explicit_library_selection_still_propagates_producer_failure(monkeypatch) -> None:
    import importlib.util

    import pytest

    spec = importlib.util.spec_from_file_location("modules_mixed", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _stub_tool_lookup(module, monkeypatch)

    def failed_library_producer(command, **kwargs):
        assert "--lib" in command
        return subprocess.CompletedProcess(command, 1, "", "library analysis failed")

    monkeypatch.setattr(module.subprocess, "run", failed_library_producer)
    with pytest.raises(RuntimeError, match="library analysis failed"):
        module.render_module_tree("mixed", "mixed")


@pytest.mark.parametrize(
    ("mode", "error"),
    [
        ("child-success", None),
        ("child-failed", "child cargo check failed"),
        ("missing", "did not prove child cargo check"),
        ("malformed", "invalid child cargo check status"),
    ],
)
def test_direct_producer_cannot_promote_tree_without_successful_child_check(
    tmp_path, monkeypatch, mode, error
) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_child_status", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    fake_cargo = fake_bin / "cargo"
    compiler_error = "error[E0425]: cannot find fixture binding"
    cargo_error = "error: could not compile the fixture"
    output = json.dumps(
        {"reason": "compiler-message", "message": {"level": "error", "rendered": compiler_error}}
    )
    fake_cargo.write_text(
        "#!/bin/sh\n"
        + (
            f"printf '%s\\n' {shlex.quote(output)}\n"
            f"printf '%s\\n' {shlex.quote(cargo_error)} >&2\n"
            "exit 101\n"
            if mode == "child-failed"
            else "exit 0\n"
        )
    )
    fake_cargo.chmod(0o700)
    fake_modules = fake_bin / "cargo-modules"
    child_call = {
        "child-success": '"$CARGO" check --workspace >/dev/null 2>&1\n',
        "child-failed": '"$CARGO" check --workspace >/dev/null 2>&1\n',
        "missing": "",
        "malformed": 'printf "bad\\n" > "$QUANTA_INDEX_MODULES_CHECK_STATUS"\n',
    }[mode]
    fake_modules.write_text(
        "#!/bin/sh\n"
        '[ "$1" = "modules" ] && [ "$2" = "structure" ] || exit 2\n'
        f"{child_call}"
        'printf "\\ncrate probe_library\\n"\n'
        "exit 0\n"
    )
    fake_modules.chmod(0o700)
    monkeypatch.setenv("PATH", f"{fake_bin}:{os.environ['PATH']}")
    monkeypatch.setenv("QUANTA_INDEX_CACHE_ROOT", str(tmp_path / "cache"))
    monkeypatch.setattr(module, "ROOT", tmp_path)
    baseline_dir = tmp_path / "baselines"
    baseline_dir.mkdir()
    baseline = baseline_dir / "probe-package.txt"
    baseline.write_text("crate previous\n")
    monkeypatch.setattr(module, "BASELINE_DIR", baseline_dir)
    monkeypatch.setattr(
        module,
        "validate_workspace_dependencies",
        lambda: {"probe-package": {"targets": [{"kind": ["lib"], "name": "probe_library"}]}},
    )
    monkeypatch.setattr(
        sys, "argv", [str(SCRIPT), "--update-baseline", "--packages", "probe-package"]
    )

    if error is None:
        assert module.main() == 0
        assert baseline.read_text() == "\ncrate probe_library\n"
    else:
        with pytest.raises(RuntimeError, match=error) as failure:
            module.main()
        assert baseline.read_text() == "crate previous\n"
        if mode == "child-failed":
            assert compiler_error in str(failure.value)
            assert cargo_error in str(failure.value)


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
    _stub_tool_lookup(module, monkeypatch)
    body = "crate quanta_index_contract\nexisting module\n"
    (tmp_path / "quanta-index-contract.txt").write_text(body)
    monkeypatch.setattr(module, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--packages", "quanta-index-contract"])
    commands = []

    def resolved(command, **kwargs):
        commands.append(command)
        if command[1] != "metadata":
            _record_child_check(kwargs)
        return subprocess.CompletedProcess(
            command,
            0,
            '{"packages":[{"id":"contract","name":"quanta-index-contract",'
            '"targets":[{"kind":["lib"],"name":"quanta_index_contract"}]}],'
            '"workspace_members":["contract"],'
            '"resolve":{"nodes":[{"id":"contract","dependencies":[]}]}}'
            if command[1] == "metadata"
            else body,
            "",
        )

    monkeypatch.setattr(module.subprocess, "run", resolved)
    assert module.main() == 0
    assert commands[0][1:] == ["metadata", "--locked", "--format-version", "1"]
    assert [Path(commands[1][0]).name, *commands[1][1:]] == [
        "cargo-modules",
        "modules",
        "structure",
        "--lib",
        "--package",
        "quanta-index-contract",
        "--no-fns",
    ]


def test_update_does_not_promote_first_tree_when_second_producer_fails(
    tmp_path, monkeypatch
) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_multi_update", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    first = tmp_path / "quanta-index-contract.txt"
    second = tmp_path / "quanta-index-core.txt"
    third = tmp_path / "quanta-index-kernel.txt"
    first.write_text("contract previous\n")
    second.write_text("core previous\n")
    third.write_text("kernel previous\n")
    monkeypatch.setattr(module, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(
        module,
        "validate_workspace_dependencies",
        lambda: _members("quanta-index-contract", "quanta-index-core", "quanta-index-kernel"),
    )
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--update-baseline"])

    def render(package, _library_name):
        if package == "quanta-index-core":
            raise RuntimeError("second module tree incomplete")
        return "crate quanta_index_contract\nnew module\n"

    monkeypatch.setattr(module, "render_module_tree", render)
    with pytest.raises(RuntimeError, match="second module tree incomplete"):
        module.main()
    assert first.read_text() == "contract previous\n"
    assert second.read_text() == "core previous\n"
    assert third.read_text() == "kernel previous\n"


def test_update_with_missing_protected_crate_does_not_promote_present_tree(
    tmp_path, monkeypatch
) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_mixed_missing", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    first = tmp_path / "quanta-index-contract.txt"
    first.write_text("contract previous\n")
    monkeypatch.setattr(module, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(
        module, "validate_workspace_dependencies", lambda: _members("quanta-index-contract")
    )
    monkeypatch.setattr(module, "render_module_tree", lambda *_: pytest.fail("producer ran"))
    monkeypatch.setattr(
        sys,
        "argv",
        [str(SCRIPT), "--update-baseline", "--packages", "quanta-index-contract", "missing"],
    )
    assert module.main() == 1
    assert first.read_text() == "contract previous\n"


def test_update_writes_all_resolved_module_trees(tmp_path, monkeypatch) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("modules_multi_success", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    monkeypatch.setattr(module, "BASELINE_DIR", tmp_path)
    monkeypatch.setattr(
        module,
        "validate_workspace_dependencies",
        lambda: _members("quanta-index-contract", "quanta-index-core", "quanta-index-kernel"),
    )
    monkeypatch.setattr(
        module,
        "render_module_tree",
        lambda package, _library_name: f"crate {package.replace('-', '_')}\n",
    )
    monkeypatch.setattr(sys, "argv", [str(SCRIPT), "--update-baseline"])
    assert module.main() == 0
    assert (tmp_path / "quanta-index-contract.txt").read_text() == "crate quanta_index_contract\n"
    assert (tmp_path / "quanta-index-core.txt").read_text() == "crate quanta_index_core\n"
    assert (tmp_path / "quanta-index-kernel.txt").read_text() == "crate quanta_index_kernel\n"
