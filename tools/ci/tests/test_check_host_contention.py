"""Tests for the local timing contention preflight."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "ci" / "timing" / "check_host_contention.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_host_contention", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_host_contention"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def idle_host() -> dict[str, object]:
    return {"os": "darwin", "cpu_count": 16, "load_average": [1.0, 1.0, 1.0]}


def test_foreign_rust_process_is_reported() -> None:
    processes = MODULE.parse_processes(
        """
        10 1 /bin/zsh just rust-timings-daemon
        11 10 python3 tools/ci/timing/check_host_contention.py
        20 1 /Users/example/.rustup/toolchains/stable/bin/cargo test --workspace
        21 20 /Users/example/.rustup/toolchains/stable/bin/rustc --crate-name demo
        """
    )

    foreign = MODULE.foreign_rust_processes(processes, 11)

    assert [process.pid for process in foreign] == [20, 21]


def test_foreign_cargo_subcommands_are_reported() -> None:
    processes = MODULE.parse_processes(
        """
        10 1 /bin/zsh just rust-timings-daemon
        11 10 python3 tools/ci/timing/check_host_contention.py
        20 1 /Users/example/.cargo/bin/cargo-mutants --package quanta-index-core
        21 1 cargo-fuzz run ipc_request_decode
        22 1 /usr/bin/python3 cargo-mutants-report.py
        """
    )

    foreign = MODULE.foreign_rust_processes(processes, 11)

    assert [process.pid for process in foreign] == [20, 21]


def test_ancestor_cargo_is_not_reported_as_foreign() -> None:
    processes = MODULE.parse_processes(
        """
        10 1 /Users/example/.rustup/toolchains/stable/bin/cargo run
        11 10 python3 tools/ci/timing/check_host_contention.py
        """
    )

    assert MODULE.foreign_rust_processes(processes, 11) == []


def test_non_rust_processes_are_ignored() -> None:
    processes = MODULE.parse_processes(
        """
        10 1 /bin/zsh just rust-timings-daemon
        11 10 python3 tools/ci/timing/check_host_contention.py
        30 1 /usr/bin/python3 worker.py
        """
    )

    assert MODULE.foreign_rust_processes(processes, 11) == []


@pytest.mark.parametrize(
    "output",
    ["", "20 1", "not-a-pid 1 cargo test", "20 1 cargo test\n20 1 rustc test"],
)
def test_invalid_process_snapshot_fails_closed(output: str) -> None:
    with pytest.raises(ValueError):
        MODULE.parse_processes(output)


def test_process_snapshot_missing_self_fails_closed(monkeypatch, tmp_path: Path, capsys) -> None:
    monkeypatch.setattr(
        MODULE.subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess(args[0], 0, "20 1 /bin/zsh\n", ""),
    )
    receipt = tmp_path / "preflight.json"

    assert MODULE.main(["--receipt", str(receipt)]) == 2
    assert json.loads(receipt.read_text(encoding="utf-8"))["status"] == "error"
    assert "invalid_process_snapshot" in capsys.readouterr().err


def test_preflight_receipt_marks_override_as_non_clean() -> None:
    foreign = [MODULE.Process(pid=20, ppid=1, command="/bin/cargo test")]
    receipt = MODULE.preflight_receipt(
        run_id="bench-local-1",
        processes=foreign,
        foreign=foreign,
        override=True,
        ps_ok=True,
        host=idle_host(),
    )

    assert receipt["status"] == "contended_override"
    assert receipt["run_id"] == "bench-local-1"
    assert receipt["foreign_rust_processes"][0]["command_sha256"].startswith("sha256:")


def test_preflight_receipt_marks_an_unsupported_host() -> None:
    receipt = MODULE.preflight_receipt(
        run_id="bench-linux-only",
        processes=[],
        foreign=[],
        override=False,
        ps_ok=True,
        expected_os="linux",
        host=idle_host(),
    )

    assert receipt["status"] == "unsupported_host"
    assert receipt["expected_os"] == "linux"


@pytest.mark.parametrize(
    ("load", "override", "expected"),
    [(8.0, False, "blocked"), (51.0, False, "blocked"), (51.0, True, "contended_override")],
)
def test_overloaded_host_never_receives_clean_status(load, override, expected) -> None:
    host = idle_host()
    host["load_average"] = [load, 1.0, 1.0]
    receipt = MODULE.preflight_receipt(
        run_id="load-test",
        processes=[],
        foreign=[],
        override=override,
        ps_ok=True,
        host=host,
    )

    assert receipt["status"] == expected
    assert receipt["host_contention"]["one_minute_load_limit"] == 8.0


@pytest.mark.parametrize(
    "invalid_host",
    [
        {"os": "darwin", "cpu_count": 16, "load_average": None},
        {"os": "darwin", "cpu_count": None, "load_average": [1.0, 1.0, 1.0]},
    ],
)
def test_missing_load_authority_is_error(invalid_host) -> None:
    receipt = MODULE.preflight_receipt(
        run_id="load-test",
        processes=[],
        foreign=[],
        override=False,
        ps_ok=True,
        host=invalid_host,
    )

    assert receipt["status"] == "error"


def test_main_blocks_overloaded_host_with_consistent_receipt(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    self_pid = MODULE.os.getpid()
    monkeypatch.setattr(
        MODULE.subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess(
            args[0], 0, f"{self_pid} 1 python3 check_host_contention.py\n", ""
        ),
    )
    host = idle_host()
    host["load_average"] = [51.0, 1.0, 1.0]
    monkeypatch.setattr(MODULE, "host_snapshot", lambda: host)
    receipt = tmp_path / "preflight.json"

    assert MODULE.main(["--receipt", str(receipt)]) == 1
    assert json.loads(receipt.read_text(encoding="utf-8"))["status"] == "blocked"
    assert "host_load" in capsys.readouterr().err


def test_receipt_write_is_valid_json_and_atomic_target(tmp_path: Path) -> None:
    target = tmp_path / "nested" / "receipt.json"
    receipt = MODULE.preflight_receipt(
        run_id=None,
        processes=[],
        foreign=[],
        override=False,
        ps_ok=True,
        host=idle_host(),
    )

    MODULE.write_receipt(target, receipt)

    assert json.loads(target.read_text(encoding="utf-8"))["status"] == "clean"
    assert not list(target.parent.glob(".receipt.json.*"))
