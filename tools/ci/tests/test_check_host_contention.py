"""Tests for the local timing contention preflight."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

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


def test_preflight_receipt_marks_override_as_non_clean() -> None:
    foreign = [MODULE.Process(pid=20, ppid=1, command="/bin/cargo test")]
    receipt = MODULE.preflight_receipt(
        run_id="bench-local-1",
        processes=foreign,
        foreign=foreign,
        override=True,
        ps_ok=True,
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
    )

    if receipt["host"]["os"] == "linux":
        assert receipt["status"] == "clean"
    else:
        assert receipt["status"] == "unsupported_host"
    assert receipt["expected_os"] == "linux"


def test_receipt_write_is_valid_json_and_atomic_target(tmp_path: Path) -> None:
    target = tmp_path / "nested" / "receipt.json"
    receipt = MODULE.preflight_receipt(
        run_id=None,
        processes=[],
        foreign=[],
        override=False,
        ps_ok=True,
    )

    MODULE.write_receipt(target, receipt)

    assert json.loads(target.read_text(encoding="utf-8"))["status"] == "clean"
    assert not list(target.parent.glob(".receipt.json.*"))
