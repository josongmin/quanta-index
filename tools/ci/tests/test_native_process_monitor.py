"""Independent mocked process custody for the native product monitor."""

from __future__ import annotations

from math import inf, nan
from pathlib import Path
from subprocess import CompletedProcess

import pytest

from tools.benchmark.retrieval import native_process_monitor as process_monitor
from tools.benchmark.retrieval.native_process_monitor import (
    MonitorError,
    NativeProcessMonitor,
    Process,
)

TEST = Path("/bound/product-test")
DAEMON = Path("/bound/searchd")
SELECTOR = "ann_quality::product_native_ann_quality_4096x64_reopens_at_fixed_recall"
CONFIG = Path("/owned/native-ann-quality.json")
DIGEST = "a" * 64


def rows(*, daemon_pid: int | None = 300, daemon_start: str = "daemon-start") -> dict[int, Process]:
    selected = Process(
        200, 100, "test-start", f"{TEST} {SELECTOR} --exact --nocapture --test-threads=1", 30,
    )
    result = {
        100: Process(100, 1, "root-start", "/bound/admission -- selected", 10),
        200: selected,
        900: Process(900, 1, "foreign-start", "/usr/bin/rustc --crate-name other", 100),
    }
    if daemon_pid is not None:
        result[daemon_pid] = Process(
            daemon_pid, 200, daemon_start, f"{DAEMON} serve-native --config {CONFIG}", 50,
        )
    return result


class Samples:
    def __init__(self, full: list[dict[int, Process]], targeted: list[dict[int, Process]]) -> None:
        self.full = full
        self.targeted = targeted
        self.full_calls = 0
        self.targeted_calls = 0
        self.targeted_pids: list[tuple[int, ...]] = []

    def all(self) -> dict[int, Process]:
        self.full_calls += 1
        return self.full.pop(0)

    def pids(self, selected: tuple[int, ...]) -> dict[int, Process]:
        self.targeted_calls += 1
        self.targeted_pids.append(selected)
        return {pid: row for pid, row in self.targeted.pop(0).items() if pid in selected}


def monitor(samples: Samples, time_now: list[float], checked: list[Path]) -> NativeProcessMonitor:
    def validate(path: Path) -> str:
        checked.append(path)
        return DIGEST

    def validate_args(args: list[str]) -> None:
        if "--diagnostic-trace" in args:
            raise MonitorError("unexpected trace")

    return NativeProcessMonitor(
        root_pid=100,
        test_executable=TEST,
        daemon_executable=DAEMON,
        selector=SELECTOR,
        validate_config=validate,
        validate_daemon_args=validate_args,
        snapshot_all=samples.all,
        snapshot_pids=samples.pids,
        clock=lambda: time_now[0],
    )


def test_targeted_stable_sampling_and_successor_discovery() -> None:
    initial = rows()
    missing_first = rows(daemon_pid=None)
    successor = rows(daemon_pid=301, daemon_start="reopen-start")
    samples = Samples(
        [initial, initial, successor],
        [initial, missing_first],
    )
    current = [0.0]
    checked: list[Path] = []
    sampler = monitor(samples, current, checked)

    for instant in (0.0, 1.0, 2.0, 30.0, 32.0):
        current[0] = instant
        sampler.sample()

    result = sampler.report()
    assert result["custody"] == "OBSERVED"
    assert result["full_snapshot_count"] == 3
    assert result["target_snapshot_count"] == 2
    assert result["compiler_sample_count"] == 2
    assert result["compiler_last"] == {"cargo": 0, "rustc": 1}
    assert result["rss_sample_count"] == 4
    assert result["rss_max_observed_aggregate_kib"] == 80
    assert result["rss_qualification"] == "NOT_RUN"
    assert len(result["daemon_epochs"]) == 2
    assert checked == [CONFIG, CONFIG]
    assert samples.targeted_pids == [(100, 200, 300)] * 2


def test_missing_daemon_rss_remains_unknown_then_invalid_config_fails_closed() -> None:
    no_daemon = rows(daemon_pid=None)
    duplicate_config = rows()
    duplicate_config[300] = Process(
        300, 200, "daemon-start",
        f"{DAEMON} serve-native --config {CONFIG} --config {CONFIG}", 50,
    )
    samples = Samples([no_daemon, duplicate_config], [])
    current = [0.0]
    checked: list[Path] = []
    sampler = monitor(samples, current, checked)
    sampler.sample()
    assert sampler.report()["rss_max_observed_aggregate_kib"] is None
    assert sampler.report()["custody"] == "NOT_OBSERVED"

    current[0] = 2.0
    with pytest.raises(MonitorError, match="config argument missing or duplicated"):
        sampler.sample()
    assert sampler.report()["custody"] == "FAILED"
    assert sampler.report()["rss_sample_count"] == 0
    assert checked == []


def test_known_pid_birth_change_refuses_without_new_config_custody() -> None:
    initial = rows()
    reused = rows(daemon_start="reused-pid-birth")
    samples = Samples([initial], [reused])
    current = [0.0]
    checked: list[Path] = []
    sampler = monitor(samples, current, checked)
    sampler.sample()

    current[0] = 2.0
    with pytest.raises(MonitorError, match="direct daemon PID/parent/start/argv identity changed"):
        sampler.sample()
    assert checked == [CONFIG]
    assert sampler.report()["custody"] == "FAILED"
    assert sampler.report()["target_snapshot_count"] == 1


def test_sparse_full_snapshot_refuses_reused_active_pid_before_rss() -> None:
    initial = rows()
    reused = rows(daemon_start="reused-pid-birth")
    samples = Samples([initial, reused], [initial])
    current = [0.0]
    checked: list[Path] = []
    sampler = monitor(samples, current, checked)
    sampler.sample()

    current[0] = 2.0
    sampler.sample()
    current[0] = 30.0
    with pytest.raises(MonitorError, match="direct daemon PID/parent/start/argv identity changed"):
        sampler.sample()
    result = sampler.report()
    assert result["custody"] == "FAILED"
    assert result["rss_sample_count"] == 2
    assert result["rss_max_observed_aggregate_kib"] == 80
    assert len(result["daemon_epochs"]) == 1
    assert checked == [CONFIG]


@pytest.mark.parametrize(
    "change",
    [
        {"test_executable": Path("relative/product-test")},
        {"daemon_executable": Path("relative/searchd")},
        {"sample_seconds": nan},
        {"sample_seconds": inf},
        {"compiler_seconds": nan},
        {"compiler_seconds": inf},
    ],
)
def test_constructor_rejects_unbound_paths_and_nonfinite_intervals(change: dict) -> None:
    options = {
        "root_pid": 100,
        "test_executable": TEST,
        "daemon_executable": DAEMON,
        "selector": SELECTOR,
        "validate_config": lambda _path: DIGEST,
    }
    options.update(change)
    with pytest.raises(ValueError, match="invalid monitor bounds"):
        NativeProcessMonitor(**options)


def test_global_ps_partial_or_malformed_output_is_not_a_foreign_count(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    outputs = [
        CompletedProcess([], 1, "100 1 10 Wed Oct 29 12:00:00 2026 /bound/admission\n", ""),
        CompletedProcess([], 0, "foreign process with embedded\nline\n", ""),
    ]
    monkeypatch.setattr(process_monitor.subprocess, "run", lambda *_args, **_kwargs: outputs.pop(0))
    with pytest.raises(MonitorError, match="snapshot unavailable"):
        process_monitor.ps_all()
    with pytest.raises(MonitorError, match="malformed process snapshot"):
        process_monitor.ps_all()


def test_successor_config_failure_does_not_accept_old_epoch() -> None:
    initial = rows()
    samples = Samples(
        [initial, rows(daemon_pid=301, daemon_start="reopen-start")],
        [rows(daemon_pid=None)],
    )
    current = [0.0]
    checked: list[Path] = []
    sampler = monitor(samples, current, checked)
    sampler.sample()
    sampler.validate_config = lambda _path: "invalid-digest"

    current[0] = 2.0
    with pytest.raises(MonitorError, match="config validator did not return"):
        sampler.sample()
    assert sampler.report()["custody"] == "FAILED"
    assert len(sampler.report()["daemon_epochs"]) == 1


